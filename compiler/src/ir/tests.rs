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
    let slot = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
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

// ---- Memory ---------------------------------------------------------------------------------------

use super::interp::{run_full, Val};

fn c(f: &mut Func, b: BlockId, n: i128) -> Value {
    f.emit(b, Op::Const(IrTy::I64, Const::Int(n)), Some(IrTy::I64))
        .unwrap()
}

#[test]
fn structs_live_in_slots_and_copy_with_memcopy() {
    // struct { i64 a; i32 b } built in one slot, copied to another, read back.
    let mut f = Func::new("f", &[], IrTy::I64);
    let e = f.entry();
    let a = f.emit(e, Op::Slot(16, 8), Some(IrTy::Ptr)).unwrap();
    let b = f.emit(e, Op::Slot(16, 8), Some(IrTy::Ptr)).unwrap();
    let ten = c(&mut f, e, 10);
    let eight = c(&mut f, e, 8);
    let off = f.emit(e, Op::PtrAdd(a, eight), Some(IrTy::Ptr)).unwrap();
    let seven = f
        .emit(e, Op::Const(IrTy::I32, Const::Int(7)), Some(IrTy::I32))
        .unwrap();
    f.emit(e, Op::Store(a, ten), None);
    f.emit(e, Op::Store(off, seven), None);
    let sixteen = c(&mut f, e, 16);
    f.emit(e, Op::MemCopy(b, a, sixteen), None);
    let eight2 = c(&mut f, e, 8);
    let boff = f.emit(e, Op::PtrAdd(b, eight2), Some(IrTy::Ptr)).unwrap();
    let x = f.emit(e, Op::Load(IrTy::I64, b), Some(IrTy::I64)).unwrap();
    let y = f
        .emit(e, Op::Load(IrTy::I32, boff), Some(IrTy::I32))
        .unwrap();
    let y64 = f
        .emit(e, Op::Cast(CastKind::SExt, y, IrTy::I64), Some(IrTy::I64))
        .unwrap();
    let sum = f
        .emit(e, Op::Bin(BinOp::Add, x, y64), Some(IrTy::I64))
        .unwrap();
    f.terminate(e, Term::Return(Some(sum)));
    let mut m = Module::default();
    m.add(f);
    assert_eq!(verify_module(&m), Vec::<String>::new());
    let out = run_full(&m, "f", &[]);
    assert_eq!(out.result, Ok(Val::int(17)));
    assert_eq!(out.leaked, 0);
}

fn heap_fn(free_it: bool, twice: bool, use_after: bool) -> Module {
    let mut f = Func::new("f", &[], IrTy::I64);
    let e = f.entry();
    let n = c(&mut f, e, 16);
    let p = f.emit(e, Op::Alloc(n), Some(IrTy::Ptr)).unwrap();
    let v = c(&mut f, e, 5);
    f.emit(e, Op::Store(p, v), None);
    if free_it {
        f.emit(e, Op::Free(p), None);
    }
    if twice {
        f.emit(e, Op::Free(p), None);
    }
    let r = if use_after {
        f.emit(e, Op::Load(IrTy::I64, p), Some(IrTy::I64)).unwrap()
    } else {
        v
    };
    f.terminate(e, Term::Return(Some(r)));
    let mut m = Module::default();
    m.add(f);
    m
}

#[test]
fn the_heap_is_checked() {
    let ok = run_full(&heap_fn(true, false, false), "f", &[]);
    assert_eq!((ok.result, ok.leaked), (Ok(Val::int(5)), 0));
    let leak = run_full(&heap_fn(false, false, false), "f", &[]);
    assert_eq!((leak.result, leak.leaked), (Ok(Val::int(5)), 16));
    let twice = run_full(&heap_fn(true, true, false), "f", &[]);
    assert_eq!(twice.result, Err("freed memory twice".to_string()));
    let after = run_full(&heap_fn(true, false, true), "f", &[]);
    assert_eq!(
        after.result,
        Err("used memory after it was freed".to_string())
    );
}

#[test]
fn a_stack_address_cannot_outlive_its_function() {
    let mut inner = Func::new("inner", &[], IrTy::Ptr);
    let e = inner.entry();
    let s = inner.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
    inner.terminate(e, Term::Return(Some(s)));
    let mut m = Module::default();
    let id = m.add(inner);
    let mut f = Func::new("f", &[], IrTy::I64);
    let e = f.entry();
    let p = f.emit(e, Op::Call(id, vec![]), Some(IrTy::Ptr)).unwrap();
    let v = f.emit(e, Op::Load(IrTy::I64, p), Some(IrTy::I64)).unwrap();
    f.terminate(e, Term::Return(Some(v)));
    m.add(f);
    let out = run_full(&m, "f", &[]);
    assert_eq!(
        out.result,
        Err("used the stack of a function that has returned".to_string())
    );
}

#[test]
fn static_data_printing_and_calls_through_pointers() {
    let mut m = Module::default();
    let d = m.add_data(b"hello\n");
    assert_eq!(m.add_data(b"hello\n"), d);
    let mut twice = Func::new("twice", &[IrTy::I64], IrTy::I64);
    let e = twice.entry();
    let x = twice.block_params(e)[0];
    let r = twice
        .emit(e, Op::Bin(BinOp::Add, x, x), Some(IrTy::I64))
        .unwrap();
    twice.terminate(e, Term::Return(Some(r)));
    let tid = m.add(twice);
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let p = f.emit(e, Op::DataAddr(d), Some(IrTy::Ptr)).unwrap();
    let six = c(&mut f, e, 6);
    f.emit(e, Op::Print(p, six), None);
    let fp = f.emit(e, Op::FuncAddr(tid), Some(IrTy::Ptr)).unwrap();
    let four = c(&mut f, e, 4);
    let r = f
        .emit(
            e,
            Op::CallPtr(vec![IrTy::I64], IrTy::I64, fp, vec![four]),
            Some(IrTy::I64),
        )
        .unwrap();
    f.terminate(e, Term::Return(Some(r)));
    m.add(f);
    assert_eq!(verify_module(&m), Vec::<String>::new());
    let out = run_full(&m, "main", &[]);
    assert_eq!(out.result, Ok(Val::int(8)));
    assert_eq!(out.output, "hello\n");
    let text = print_module(&m);
    assert!(
        text.contains("data d0")
            && text.contains("callptr (i64) -> i64, v")
            && text.contains("funcaddr @twice"),
        "{text}"
    );
}

#[test]
fn memory_ops_are_type_checked() {
    let mut f = Func::new("f", &[IrTy::I64], IrTy::Void);
    let x = f.block_params(f.entry())[0];
    let e = f.entry();
    f.emit(e, Op::PtrAdd(x, x), Some(IrTy::Ptr));
    f.emit(e, Op::MemCopy(x, x, x), None);
    f.terminate(e, Term::Return(None));
    let errs = verify_func(&f);
    assert!(
        errs.iter()
            .any(|m| m.contains("`ptradd` needs a ptr and an i64")),
        "{errs:?}"
    );
    assert!(
        errs.iter().any(|m| m.contains("`memcopy` needs two ptrs")),
        "{errs:?}"
    );
}
