//! IR tests: hand-built functions, their printed form, and the verifier.

use super::*;

/// `sum(n)`: adds 0 + 1 + ... + (n - 1), stopping on overflow.
fn sum_to() -> Func {
    let mut f = Func::new("sum_to", &[IrTy::I64], IrTy::I64);
    let n = f.block_params(f.entry())[0];
    let b0 = f.entry();
    let zero = f
        .emit(b0, Op::Const(IrTy::I64, Const::Int(0)), Some(IrTy::I64))
        .unwrap();
    let head = f.new_block(&[IrTy::I64, IrTy::I64]);
    let body = f.new_block(&[]);
    let done = f.new_block(&[IrTy::I64]);
    f.terminate(
        b0,
        Term::Jump(Target {
            block: head,
            args: vec![zero, zero],
        }),
    );
    let (acc, i) = (f.block_params(head)[0], f.block_params(head)[1]);
    let more = f
        .emit(head, Op::Cmp(CmpOp::Lt, i, n), Some(IrTy::Bool))
        .unwrap();
    f.terminate(
        head,
        Term::Branch(
            more,
            Target {
                block: body,
                args: vec![],
            },
            Target {
                block: done,
                args: vec![acc],
            },
        ),
    );
    let acc2 = f
        .emit(body, Op::Checked(BinOp::Add, true, acc, i), Some(IrTy::I64))
        .unwrap();
    let one = f
        .emit(body, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    let i2 = f
        .emit(body, Op::Bin(BinOp::Add, i, one), Some(IrTy::I64))
        .unwrap();
    f.terminate(
        body,
        Term::Jump(Target {
            block: head,
            args: vec![acc2, i2],
        }),
    );
    let r = f.block_params(done)[0];
    f.terminate(done, Term::Return(Some(r)));
    f
}

#[test]
fn a_loop_prints_and_verifies() {
    let f = sum_to();
    assert_eq!(verify_func(&f), Vec::<String>::new());
    let text = print_func(&f, None);
    assert_eq!(
        text,
        "fn sum_to(v0: i64) -> i64 {
  b0:
    v1 = const i64 0
    jump b1(v1, v1)
  b1(v2: i64, v3: i64):
    v5 = lt v3, v0
    branch v5, b2, b3(v2)
  b2:
    v6 = add_checked v2, v3
    v7 = const i64 1
    v8 = add v3, v7
    jump b1(v6, v8)
  b3(v4: i64):
    return v4
}
"
    );
}

#[test]
fn calls_are_checked_against_the_callee() {
    let mut m = Module::default();
    let callee = m.add(sum_to());
    let mut f = Func::new("main", &[], IrTy::I64);
    let b0 = f.entry();
    let ten = f
        .emit(b0, Op::Const(IrTy::I64, Const::Int(10)), Some(IrTy::I64))
        .unwrap();
    let r = f
        .emit(b0, Op::Call(callee, vec![ten]), Some(IrTy::I64))
        .unwrap();
    f.terminate(b0, Term::Return(Some(r)));
    m.add(f);
    assert_eq!(verify_module(&m), Vec::<String>::new());
    assert!(print_module(&m).contains("call @sum_to(v0)"));

    // Wrong argument count and type.
    let mut g = Func::new("bad", &[], IrTy::Void);
    let b = g.entry();
    let t = g
        .emit(
            b,
            Op::Const(IrTy::Bool, Const::Bool(true)),
            Some(IrTy::Bool),
        )
        .unwrap();
    g.emit(b, Op::Call(callee, vec![t]), Some(IrTy::I64));
    g.emit(b, Op::Call(callee, vec![]), Some(IrTy::I64));
    g.terminate(b, Term::Return(None));
    m.add(g);
    let errs = verify_module(&m);
    assert!(
        errs.iter()
            .any(|e| e.contains("argument 1 of `sum_to` has the wrong type")),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| e.contains("takes 1 arguments, but 0 were given")),
        "{errs:?}"
    );
}

fn flagged(f: &Func, needle: &str) {
    let errs = verify_func(f);
    assert!(
        errs.iter().any(|e| e.contains(needle)),
        "expected {needle:?}, got {errs:?}"
    );
}

#[test]
fn the_verifier_finds_mistakes() {
    // No terminator.
    let f = Func::new("f", &[], IrTy::Void);
    flagged(&f, "has no terminator");

    // Operand types differ.
    let mut f = Func::new("f", &[IrTy::I64, IrTy::Bool], IrTy::Void);
    let (a, b) = (f.block_params(f.entry())[0], f.block_params(f.entry())[1]);
    let e = f.entry();
    f.emit(e, Op::Bin(BinOp::Add, a, b), Some(IrTy::I64));
    f.terminate(e, Term::Return(None));
    flagged(&f, "needs operands of one type");

    // Return type.
    let mut f = Func::new("f", &[], IrTy::I64);
    let e = f.entry();
    f.terminate(e, Term::Return(None));
    flagged(&f, "`return` needs a i64 value");

    // Branch needs a bool, and block arguments must match.
    let mut f = Func::new("f", &[IrTy::I64], IrTy::Void);
    let a = f.block_params(f.entry())[0];
    let e = f.entry();
    let next = f.new_block(&[IrTy::I64]);
    f.terminate(
        e,
        Term::Branch(
            a,
            Target {
                block: next,
                args: vec![],
            },
            Target {
                block: next,
                args: vec![a],
            },
        ),
    );
    f.terminate(next, Term::Return(None));
    flagged(&f, "`branch` needs a bool");
    flagged(&f, "b1 takes 1 values, but 0 were given");

    // Unreachable block.
    let mut f = Func::new("f", &[], IrTy::Void);
    let e = f.entry();
    let lone = f.new_block(&[]);
    f.terminate(e, Term::Return(None));
    f.terminate(lone, Term::Return(None));
    flagged(&f, "b1 cannot be reached");
}

#[test]
fn a_value_must_be_made_before_it_is_used() {
    // Used before it is made in the same block.
    let mut f = Func::new("f", &[], IrTy::Void);
    let e = f.entry();
    let ghost = f.new_value(IrTy::I64);
    f.emit(e, Op::Neg(ghost), Some(IrTy::I64));
    f.terminate(e, Term::Return(None));
    flagged(&f, "never made");

    // Made on one side of a branch, used after the join.
    let mut f = Func::new("f", &[IrTy::Bool], IrTy::I64);
    let c = f.block_params(f.entry())[0];
    let e = f.entry();
    let left = f.new_block(&[]);
    let right = f.new_block(&[]);
    let join = f.new_block(&[]);
    f.terminate(
        e,
        Term::Branch(
            c,
            Target {
                block: left,
                args: vec![],
            },
            Target {
                block: right,
                args: vec![],
            },
        ),
    );
    let x = f
        .emit(left, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    f.terminate(
        left,
        Term::Jump(Target {
            block: join,
            args: vec![],
        }),
    );
    f.terminate(
        right,
        Term::Jump(Target {
            block: join,
            args: vec![],
        }),
    );
    f.terminate(join, Term::Return(Some(x)));
    flagged(&f, "before it is made");

    // The same value handed through a block parameter is fine.
    let mut f = Func::new("f", &[IrTy::Bool], IrTy::I64);
    let c = f.block_params(f.entry())[0];
    let e = f.entry();
    let left = f.new_block(&[]);
    let right = f.new_block(&[]);
    let join = f.new_block(&[IrTy::I64]);
    f.terminate(
        e,
        Term::Branch(
            c,
            Target {
                block: left,
                args: vec![],
            },
            Target {
                block: right,
                args: vec![],
            },
        ),
    );
    let one = f
        .emit(left, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    let two = f
        .emit(right, Op::Const(IrTy::I64, Const::Int(2)), Some(IrTy::I64))
        .unwrap();
    f.terminate(
        left,
        Term::Jump(Target {
            block: join,
            args: vec![one],
        }),
    );
    f.terminate(
        right,
        Term::Jump(Target {
            block: join,
            args: vec![two],
        }),
    );
    let r = f.block_params(join)[0];
    f.terminate(join, Term::Return(Some(r)));
    assert_eq!(verify_func(&f), Vec::<String>::new());
}

#[test]
fn memory_and_casts() {
    let mut f = Func::new("f", &[IrTy::Int(32)], IrTy::I64);
    let x = f.block_params(f.entry())[0];
    let e = f.entry();
    let slot = f.emit(e, Op::Slot(IrTy::I64), Some(IrTy::Ptr)).unwrap();
    let wide = f
        .emit(e, Op::Cast(CastKind::SExt, x, IrTy::I64), Some(IrTy::I64))
        .unwrap();
    f.emit(e, Op::Store(slot, wide), None);
    let back = f
        .emit(e, Op::Load(IrTy::I64, slot), Some(IrTy::I64))
        .unwrap();
    f.terminate(e, Term::Return(Some(back)));
    assert_eq!(verify_func(&f), Vec::<String>::new());
    let text = print_func(&f, None);
    assert!(
        text.contains("sext v0 to i64")
            && text.contains("store v1, v2")
            && text.contains("load i64, v1"),
        "{text}"
    );

    // A narrowing "extension" is wrong.
    let mut g = Func::new("g", &[IrTy::I64], IrTy::Void);
    let y = g.block_params(g.entry())[0];
    let e = g.entry();
    g.emit(
        e,
        Op::Cast(CastKind::ZExt, y, IrTy::Int(32)),
        Some(IrTy::Int(32)),
    );
    g.terminate(e, Term::Return(None));
    flagged(&g, "`zext` does not fit these types");
}

#[test]
fn stop_and_unreachable_end_blocks() {
    let mut f = Func::new("f", &[], IrTy::Void);
    let e = f.entry();
    f.terminate(e, Term::Stop("index out of range".to_string()));
    assert_eq!(verify_func(&f), Vec::<String>::new());
    assert!(print_func(&f, None).contains("stop \"index out of range\""));
}
