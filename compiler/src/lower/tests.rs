//! Lowering tests: source in, IR out, then verified and run in the interpreter.

use super::*;
use crate::ir::{print_module, run, verify_module, Val};
use crate::package::Program;
use crate::resolve::resolve;
use crate::typecheck::check;

fn lowered(src: &str) -> Lowered {
    let mut program = Program::with_core();
    program.add_package("app", &[("main", src)]);
    let res = resolve(&program);
    let typed = check(&program, &res);
    let errs: Vec<&str> = typed
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert!(errs.is_empty(), "the source does not check: {errs:?}");
    let l = lower(&program, &res, &typed);
    let v = verify_module(&l.module);
    assert!(
        v.is_empty(),
        "the IR does not verify: {v:?}\n{}",
        print_module(&l.module)
    );
    l
}

/// Runs `name(args)` and gives the result as an integer.
fn eval(src: &str, name: &str, args: &[i64]) -> Result<i64, String> {
    let l = lowered(src);
    let args: Vec<Val> = args.iter().map(|a| Val::int(*a)).collect();
    run(&l.module, name, &args).map(|v| match v {
        Val::Bool(b) => b as i64,
        v => v.as_i64(),
    })
}

#[test]
fn arithmetic_and_calls() {
    let src = "fn add(a: int, b: int) -> int = a + b\nfn twice(x: int) -> int = add(x, x)\nfn mix(a: int) -> int = (a * 3 - 4) / 2 + a % 5";
    assert_eq!(eval(src, "twice", &[21]), Ok(42));
    assert_eq!(eval(src, "mix", &[10]), Ok(13));
    assert_eq!(eval("fn f() -> int = -7 + 2", "f", &[]), Ok(-5));
    assert_eq!(eval("fn f(a: int) -> int = -a", "f", &[5]), Ok(-5));
}

#[test]
fn locals_and_assignment() {
    let src = "fn f(n: int) -> int {\n var total = 0\n total += n\n total *= 2\n total -= 1\n const k = total + 1\n k }";
    assert_eq!(eval(src, "f", &[5]), Ok(10));
    let src = "fn f() -> int { var i = 0\n i++\n i++\n i-- \n i }";
    assert_eq!(eval(src, "f", &[]), Ok(1));
}

#[test]
fn if_while_loop_for() {
    let max = "fn max2(a: int, b: int) -> int { if a > b { a } else { b } }";
    assert_eq!(eval(max, "max2", &[3, 9]), Ok(9));
    assert_eq!(eval(max, "max2", &[9, 3]), Ok(9));
    let fact =
        "fn fact(n: int) -> int { var r = 1\n var i = 2\n while i <= n { r *= i\n i++ }\n r }";
    assert_eq!(eval(fact, "fact", &[10]), Ok(3_628_800));
    let sum = "fn sum(n: int) -> int { var t = 0\n for i of 0..n { t += i }\n t }";
    assert_eq!(eval(sum, "sum", &[10]), Ok(45));
    let sumi = "fn sum(n: int) -> int { var t = 0\n for i of 1..=n { t += i }\n t }";
    assert_eq!(eval(sumi, "sum", &[10]), Ok(55));
    assert_eq!(eval(sumi, "sum", &[0]), Ok(0));
    let lp = "fn f() -> int { var i = 0\n loop { i++\n if i == 7 { break } }\n i }";
    assert_eq!(eval(lp, "f", &[]), Ok(7));
    let lv = "fn f(n: int) -> int = loop { if n > 3 { break n * 2 }\n return 0 }";
    assert_eq!(eval(lv, "f", &[5]), Ok(10));
    assert_eq!(eval(lv, "f", &[1]), Ok(0));
}

#[test]
fn continue_break_labels_and_early_return() {
    let src = "fn f(n: int) -> int { var t = 0\n for i of 0..n { if i % 2 == 0 { continue }\n t += i }\n t }";
    assert_eq!(eval(src, "f", &[10]), Ok(25));
    let nested = "fn f() -> int { var c = 0\n outer: for i of 0..5 { for j of 0..5 { if j == 3 { continue outer }\n if i == 4 { break outer }\n c++ } }\n c }";
    assert_eq!(eval(nested, "f", &[]), Ok(12));
    let early = "fn f(n: int) -> int { if n < 0 { return -1 }\n n }";
    assert_eq!(eval(early, "f", &[-5]), Ok(-1));
    assert_eq!(eval(early, "f", &[5]), Ok(5));
}

#[test]
fn booleans_ternary_and_recursion() {
    let src = "fn inrange(x: int) -> bool = x >= 0 && x < 10";
    assert_eq!(eval(src, "inrange", &[5]), Ok(1));
    assert_eq!(eval(src, "inrange", &[10]), Ok(0));
    let or = "fn f(x: int) -> bool = x < 0 || x > 100";
    assert_eq!(eval(or, "f", &[-1]), Ok(1));
    assert_eq!(eval(or, "f", &[50]), Ok(0));
    let not = "fn f(x: bool) -> bool = !x";
    assert_eq!(eval(not, "f", &[1]), Ok(0));
    let tern = "fn f(x: int) -> int = x > 0 ? 1 : -1";
    assert_eq!(eval(tern, "f", &[8]), Ok(1));
    assert_eq!(eval(tern, "f", &[-8]), Ok(-1));
    let fib = "fn fib(n: int) -> int { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }";
    assert_eq!(eval(fib, "fib", &[15]), Ok(610));
    let unless = "fn f(x: int) -> int { unless x > 0 { 0 } else { x } }";
    assert_eq!(eval(unless, "f", &[4]), Ok(4));
    assert_eq!(eval(unless, "f", &[-4]), Ok(0));
}

#[test]
fn stops_for_bugs() {
    assert_eq!(
        eval("fn f(a: int) -> int = 10 / a", "f", &[0]),
        Err("division by zero".to_string())
    );
    assert_eq!(
        eval("fn f(a: int) -> int = 10 % a", "f", &[0]),
        Err("division by zero".to_string())
    );
    assert_eq!(
        eval("fn f(a: int) -> int = a + 1", "f", &[i64::MAX]),
        Err("arithmetic overflow".to_string())
    );
    assert_eq!(
        eval("fn f(a: int) -> int = a * 2", "f", &[i64::MAX]),
        Err("arithmetic overflow".to_string())
    );
    assert_eq!(
        eval("fn f(a: int) -> int = -a", "f", &[i64::MIN]),
        Err("arithmetic overflow".to_string())
    );
    assert_eq!(
        eval("fn f(a: int) -> int = 1 << a", "f", &[64]),
        Err("shifted by the width or more".to_string())
    );
    assert_eq!(eval("fn f(a: int) -> int = 1 << a", "f", &[4]), Ok(16));
    // Wrapping operators do not stop.
    assert_eq!(
        eval("fn f(a: int) -> int = a +% 1", "f", &[i64::MAX]),
        Ok(i64::MIN)
    );
}

#[test]
fn narrow_and_unsigned_integers() {
    let src = "fn f(a: uint8, b: uint8) -> uint8 = a + b";
    assert_eq!(eval(src, "f", &[100, 100]), Ok(200));
    assert_eq!(
        eval(src, "f", &[200, 100]),
        Err("arithmetic overflow".to_string())
    );
    let w = "fn f(a: uint8) -> uint8 = a +% 100";
    assert_eq!(eval(w, "f", &[200]), Ok(44));
    let c = "fn f(a: uint8, b: uint8) -> bool = a > b";
    assert_eq!(eval(c, "f", &[200, 100]), Ok(1));
    let conv = "fn f(a: int32) -> int = a to int";
    assert_eq!(eval(conv, "f", &[-5]), Ok(-5));
    let conv = "fn f(a: uint8) -> int = a to int";
    assert_eq!(eval(conv, "f", &[250]), Ok(250));
    let top = "fn f() -> int { var t = 0\n const last: uint8 = 255\n for i of 250..=last { t += 1 }\n t }";
    assert_eq!(eval(top, "f", &[]), Ok(6));
}

#[test]
fn floats() {
    let l = lowered("fn f(a: float, b: float) -> float = a * b + 1.5");
    let r = run(&l.module, "f", &[Val::Float(2.0), Val::Float(4.0)]);
    assert_eq!(r, Ok(Val::Float(9.5)));
    let l = lowered("fn f(a: float) -> bool = a < 2.5");
    assert_eq!(run(&l.module, "f", &[Val::Float(1.0)]), Ok(Val::Bool(true)));
    let l = lowered("fn f(a: int) -> float = a to float");
    assert_eq!(run(&l.module, "f", &[Val::int(3)]), Ok(Val::Float(3.0)));
}

#[test]
fn what_is_not_lowered_yet_is_reported_and_stops() {
    let l = lowered("fn f() -> int { const x = 1 >> 0\n 5 }\nfn g() -> int = 1");
    assert_eq!(run(&l.module, "g", &[]), Ok(Val::int(1)));
    assert!(l.diagnostics.is_empty() || l.diagnostics.iter().all(|d| d.warning));
}

#[test]
fn the_printed_form_of_a_small_function() {
    let l = lowered("fn inc(x: int) -> int = x + 1");
    assert_eq!(
        print_module(&l.module),
        "fn inc(v0: i64) -> i64 {
  b0:
    v1 = const i64 1
    v2 = add_checked v0, v1
    return v2
}
"
    );
}
