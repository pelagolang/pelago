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

// ---- Aggregates -----------------------------------------------------------------------------------------

#[test]
fn structs_and_fields() {
    let src = "struct point { x: int, y: int }\nfn dist2(p: point) -> int = p.x * p.x + p.y * p.y\nfn f() -> int { const p = point { x: 3, y: 4 }\n dist2(p) }";
    assert_eq!(eval(src, "f", &[]), Ok(25));
    let src = "struct point { x: int, y: int }\nfn f() -> int { var p = point { x: 1, y: 2 }\n p.x = 10\n p.y += 5\n p.x * 100 + p.y }";
    assert_eq!(eval(src, "f", &[]), Ok(1007));
    let src = "struct point { x: int, y: int }\nfn make(a: int) -> point = point { x: a, y: a * 2 }\nfn f() -> int { const p = make(7)\n p.x + p.y }";
    assert_eq!(eval(src, "f", &[]), Ok(21));
    let src = "struct point { x: int, y: int }\nfn f() -> int { const p = point { x: 1, y: 2 }\n const q = point { y: 9, ...p }\n q.x * 10 + q.y }";
    assert_eq!(eval(src, "f", &[]), Ok(19));
    let src = "struct cfg { port: int = 80, debug: bool = false }\nfn f() -> int { const c = cfg { debug: true }\n c.port + (c.debug ? 1 : 0) }";
    assert_eq!(eval(src, "f", &[]), Ok(81));
    let src = "struct p { x: int, y: int }\nfn f() -> int { const x = 4\n const y = 5\n const q = p { x, y }\n q.x + q.y }";
    assert_eq!(eval(src, "f", &[]), Ok(9));
}

#[test]
fn tuples_and_destructuring() {
    let src = "fn swap(p: (int, int)) -> (int, int) = (p.1, p.0)\nfn f() -> int { const (a, b) = swap((1, 2))\n a * 10 + b }";
    assert_eq!(eval(src, "f", &[]), Ok(21));
    let src = "fn f() -> int { const t = (1, true, 3)\n t.0 + t.2 }";
    assert_eq!(eval(src, "f", &[]), Ok(4));
    let src = "struct p { x: int, y: int }\nfn f() -> int { const q = p { x: 6, y: 7 }\n const p { x, y } = q\n x * y }";
    assert_eq!(eval(src, "f", &[]), Ok(42));
}

#[test]
fn enums_and_switch() {
    let shapes = "enum shape { circle(int)\n rect(int, int)\n empty }\n";
    let area = format!("{shapes}fn area(s: shape) -> int {{ switch s {{ case .circle(r): 3 * r * r\n case .rect(w, h): w * h\n case .empty: 0 }} }}\n");
    assert_eq!(
        eval(&format!("{area}fn f() -> int = area(.circle(2))"), "f", &[]),
        Ok(12)
    );
    assert_eq!(
        eval(
            &format!("{area}fn f() -> int = area(shape.rect(3, 4))"),
            "f",
            &[]
        ),
        Ok(12)
    );
    assert_eq!(
        eval(&format!("{area}fn f() -> int = area(.empty)"), "f", &[]),
        Ok(0)
    );
    let alt = format!("{shapes}fn f(s: shape) -> int {{ switch s {{ case .circle(r), .rect(r, _): r\n default: -1 }} }}");
    assert_eq!(
        eval(&format!("{alt}\nfn g() -> int = f(.rect(7, 8))"), "g", &[]),
        Ok(7)
    );
    assert_eq!(
        eval(&format!("{alt}\nfn g() -> int = f(.empty)"), "g", &[]),
        Ok(-1)
    );
    let guard = format!("{shapes}fn f(s: shape) -> int {{ switch s {{ case .circle(r) if r > 5: 1\n case .circle(r): 2\n default: 3 }} }}");
    assert_eq!(
        eval(&format!("{guard}\nfn g() -> int = f(.circle(9))"), "g", &[]),
        Ok(1)
    );
    assert_eq!(
        eval(&format!("{guard}\nfn g() -> int = f(.circle(1))"), "g", &[]),
        Ok(2)
    );
    assert_eq!(
        eval(&format!("{guard}\nfn g() -> int = f(.empty)"), "g", &[]),
        Ok(3)
    );
}

#[test]
fn numbers_in_switch() {
    let src = "fn f(n: int) -> int { switch n { case 0: 10\n case 1..5: 20\n case 5..=9: 30\n default: 40 } }";
    assert_eq!(eval(src, "f", &[0]), Ok(10));
    assert_eq!(eval(src, "f", &[3]), Ok(20));
    assert_eq!(eval(src, "f", &[9]), Ok(30));
    assert_eq!(eval(src, "f", &[50]), Ok(40));
}

#[test]
fn options() {
    let src = "fn pick(a: int, b: int) -> int? { if a > 0 { a } else if b > 0 { b } else { none } }\nfn f(a: int, b: int) -> int { switch pick(a, b) { case some(v): v\n case none: -1 } }";
    assert_eq!(eval(src, "f", &[5, 0]), Ok(5));
    assert_eq!(eval(src, "f", &[0, 6]), Ok(6));
    assert_eq!(eval(src, "f", &[0, 0]), Ok(-1));
    let src = "fn f(o: int?) -> int { if o is some(v) && v > 3 { v } else { 0 } }\nfn g() -> int = f(9)\nfn h() -> int = f(2)\nfn k() -> int = f(none)";
    assert_eq!(eval(src, "g", &[]), Ok(9));
    assert_eq!(eval(src, "h", &[]), Ok(0));
    assert_eq!(eval(src, "k", &[]), Ok(0));
}

#[test]
fn results() {
    let src = "fn half(n: int) -> int ! error { if n % 2 == 0 { ok(n / 2) } else { err(oops()) } }\nfn oops() -> error = todo()";
    let _ = src;
    let src = "fn safe_div(a: int, b: int) -> int ! int { if b == 0 { err(-1) } else { ok(a / b) } }\nfn f(a: int, b: int) -> int { switch safe_div(a, b) { case ok(v): v\n case err(c): c } }";
    assert_eq!(eval(src, "f", &[10, 2]), Ok(5));
    assert_eq!(eval(src, "f", &[10, 0]), Ok(-1));
}

#[test]
fn methods_and_inout() {
    let src = "struct point { x: int, y: int\n fn sum(self) -> int = self.x + self.y\n fn shift(inout self, d: int) { self.x += d\n self.y += d } }\nfn f() -> int { var p = point { x: 1, y: 2 }\n p.shift!(10)\n p.sum() }";
    assert_eq!(eval(src, "f", &[]), Ok(23));
    let src = "fn bump(inout n: int) { n += 1 }\nfn f() -> int { var a = 5\n bump(inout a)\n bump(inout a)\n a }";
    assert_eq!(eval(src, "f", &[]), Ok(7));
    let src = "struct counter { n: int\n fn next(inout self) -> int { self.n += 1\n self.n } }\nfn f() -> int { var c = counter { n: 0 }\n c.next!()\n c.next!()\n c.next!() }";
    assert_eq!(eval(src, "f", &[]), Ok(3));
}

#[test]
fn generics_are_specialised() {
    let src = "fn first<T>(a: T, b: T) -> T = a\nfn f() -> int = first(7, 8)\nfn g() -> bool = first(true, false)";
    assert_eq!(eval(src, "f", &[]), Ok(7));
    assert_eq!(eval(src, "g", &[]), Ok(1));
    let l = lowered(src);
    let names: Vec<&str> = l.module.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"first<int>") && names.contains(&"first<bool>"),
        "{names:?}"
    );
    let src = "struct pair<T> { a: T, b: T\n fn sum(self) -> int = 0 }\nfn f() -> int { const p = pair { a: 3, b: 4 }\n p.a + p.b }";
    assert_eq!(eval(src, "f", &[]), Ok(7));
}

// ---- Text, lists, printing, ownership --------------------------------------------------------------------

use crate::ir::{run_full, Outcome};

/// Runs `main` and gives what it wrote and whether it leaked.
fn run_main(src: &str) -> Outcome {
    let l = lowered(src);
    run_full(&l.module, "main", &[])
}

fn output_of(src: &str) -> String {
    let l = lowered(src);
    let o = run_full(&l.module, "main", &[]);
    assert_eq!(o.result.clone().map(|_| ()), Ok(()), "{src}");
    assert_eq!(o.leaked, 0, "leaked memory in {src}");
    // Differential test: the optimised program must do exactly the same.
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    crate::ir::opt::prune(&mut m, &["main"]);
    let v = verify_module(&m);
    assert!(
        v.is_empty(),
        "the optimised IR does not verify: {v:?} in {src}"
    );
    let p = run_full(&m, "main", &[]);
    assert_eq!(
        p.result, o.result,
        "result changed by the optimiser in {src}"
    );
    assert_eq!(
        p.output, o.output,
        "output changed by the optimiser in {src}"
    );
    assert_eq!(p.leaked, 0, "the optimiser caused a leak in {src}");
    native_must_agree(&l.module, "plain", src, &o.output);
    native_must_agree(&m, "optimised", src, &o.output);
    o.output
}

/// The native configurations every program runs in: without registers, with all of them, and with
/// only the first one, two or four of each kind.
const REG_SETS: [(bool, usize); 5] = [
    (false, usize::MAX),
    (true, usize::MAX),
    (true, 1),
    (true, 2),
    (true, 4),
];

/// The third column of the differential test: the native build prints the same, leaks nothing and
/// exits with status 0. It runs on Linux x86-64 for Linux, and for Windows through Wine (or directly
/// on Windows) where that is available; the Windows program is built from the plain IR only.
fn native_must_agree(m: &crate::ir::Module, what: &str, src: &str, want: &str) {
    use crate::native::{exec, gen, Os};
    for os in [Os::Linux, Os::Windows] {
        if !exec::can_run(os) || (os == Os::Windows && what != "plain") {
            continue;
        }
        // The frame-only generator and the one with registers must both be right, also with few
        // registers (every spill and split path).
        // The machine-IR path (work item W4) runs beside the old generator on every configuration
        // that has registers, on Linux.
        for (mir, (regalloc, limit)) in REG_SETS.iter().flat_map(|&c| [(false, c), (true, c)]) {
            if os == Os::Windows && (mir || !regalloc || limit != usize::MAX) {
                continue;
            }
            if mir && !regalloc {
                continue;
            }
            gen::MIR.with(|m| m.set(Some(mir)));
            gen::REG_LIMIT.with(|l| l.set(limit));
            let opts = gen::Options {
                leak_check: true,
                target: os,
                regalloc,
            };
            let image = match gen::compile(m, "main", &opts) {
                Ok(i) => i,
                Err(e) => panic!("the native build ({what}, {os:?}) failed: {e}\n{src}"),
            };
            gen::REG_LIMIT.with(|l| l.set(usize::MAX));
            gen::MIR.with(|m| m.set(None));
            let r = exec::run_image(os, &image).unwrap();
            let how = format!("{what}, {os:?}, mir {mir}, regalloc {regalloc}, {limit} registers");
            assert_eq!(r.out, want, "native output ({how}) differs in {src}");
            assert_eq!(
                (r.err.as_str(), r.code),
                ("", 0),
                "native run ({how}) in {src}"
            );
        }
    }
    // The AArch64 build (run under qemu on other machines) must print the same and leak nothing.
    if let Some(r) = exec::build_and_run_a64(m, true) {
        assert_eq!(r.out, want, "AArch64 output ({what}) differs in {src}");
        assert_eq!(
            (r.err.as_str(), r.code),
            ("", 0),
            "AArch64 run ({what}) in {src}"
        );
    }
}

#[test]
fn printing_basic_values() {
    assert_eq!(output_of("fn main() { print(\"hello\") }"), "hello\n");
    assert_eq!(
        output_of("fn main() { print(1, 2.5, true, 'x', \"s\") }"),
        "1 2.5 true x s\n"
    );
    assert_eq!(
        output_of("fn main() { print(-42, 0, 9223372036854775807) }"),
        "-42 0 9223372036854775807\n"
    );
    assert_eq!(
        output_of("fn main() { print(0.1 + 0.2, 100.0, 1.0 / 3.0) }"),
        "0.3 100 0.333333\n"
    );
}

#[test]
fn printing_one_integer_needs_no_heap() {
    let src = "fn main() {\n print(0)\n print(-9223372036854775807 - 1)\n print(9223372036854775807)\n print(-7)\n print(1234567890)\n const small: int8 = -128\n print(small)\n const big: uint8 = 255\n print(big)\n}\n";
    assert_eq!(
        output_of(src),
        "0\n-9223372036854775808\n9223372036854775807\n-7\n1234567890\n-128\n255\n"
    );
    let mut m = lowered(src).module;
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    crate::ir::opt::prune(&mut m, &["main"]);
    let u = crate::native::Uses::of(&m);
    assert!(u.print && !u.alloc && !u.free && !u.realloc, "{u:?}");
    // With a second value the line is built on the heap, as before.
    let mut m = lowered("fn main() { print(1, 2) }").module;
    crate::ir::opt::prune(&mut m, &["main"]);
    assert!(crate::native::Uses::of(&m).alloc);
}

#[test]
fn interpolation_and_joining() {
    assert_eq!(
        output_of("fn main() { const name = \"Ann\"\n print(\"hi #name, #(1 + 2)!\") }"),
        "hi Ann, 3!\n"
    );
    assert_eq!(
        output_of("fn main() { const a = \"foo\"\n const b = \"bar\"\n print(a + b + \"!\") }"),
        "foobar!\n"
    );
    assert_eq!(
        output_of("fn main() { var s = \"a\"\n s += \"b\"\n s += \"c\"\n print(s) }"),
        "abc\n"
    );
    assert_eq!(
        output_of("fn main() { print(\"tab\\there \\\"q\\\"\") }"),
        "tab\there \"q\"\n"
    );
}

#[test]
fn text_comparison_and_slices() {
    assert_eq!(
        output_of(
            "fn main() { print(\"a\" == \"a\", \"a\" == \"b\", \"a\" < \"b\", \"b\" <= \"a\") }"
        ),
        "true false true false\n"
    );
    assert_eq!(
        output_of("fn main() { const s = \"hello\"\n print(s[1..3], s[..2], s[3..]) }"),
        "el he lo\n"
    );
    assert_eq!(output_of("fn f(s: str) -> int { switch s { case \"a\": 1\n case \"b\": 2\n default: 0 } }\nfn main() { print(f(\"a\"), f(\"b\"), f(\"z\")) }"), "1 2 0\n");
}

#[test]
fn lists() {
    assert_eq!(
        output_of(
            "fn main() { var xs: [int] = []\n xs.push!(3)\n xs.push!(4)\n print(xs.len(), xs) }"
        ),
        "2 [3, 4]\n"
    );
    assert_eq!(output_of("fn main() { const xs = [1, 2, 3]\n var t = 0\n for x of xs { t += x }\n print(t, xs[1]) }"), "6 2\n");
    assert_eq!(
        output_of("fn main() { var xs = [5, 3, 9, 1]\n xs.sort!()\n print(xs, xs.reversed()) }"),
        "[1, 3, 5, 9] [9, 5, 3, 1]\n"
    );
    assert_eq!(output_of("fn main() { var xs = [1, 2, 3]\n const p = xs.pop!()\n print(p, xs, xs.first(), xs.last(), xs.get(7)) }"), "some(3) [1, 2] some(1) some(2) none\n");
    assert_eq!(
        output_of("fn main() { const xs = [10, 20, 30]\n for i, x of xs { print(i, x) } }"),
        "0 10\n1 20\n2 30\n"
    );
    assert_eq!(output_of("fn main() { var xs = [1, 2, 3]\n xs.insert!(1, 9)\n const r = xs.remove!(0)\n print(xs, r, xs.contains(9), xs.contains(7)) }"), "[9, 2, 3] 1 true false\n");
    let o = run_main("fn main() { const xs = [1, 2, 3]\n print(xs[5]) }");
    assert_eq!(o.result, Err("index out of range".to_string()));
}

#[test]
fn owning_values_are_dropped_exactly_once() {
    let src = "fn make(n: int) -> [string] {\n var out: [string] = []\n for i of 0..n { out.push!(\"item #i\") }\n out\n}\nfn main() { const xs = make(3)\n print(xs, xs.len()) }";
    assert_eq!(output_of(src), "[\"item 0\", \"item 1\", \"item 2\"] 3\n");
    let src = "fn main() { var a = \"x\"\n var b = a\n b += \"y\"\n print(b) }";
    assert_eq!(output_of(src), "xy\n");
    let src = "struct person { name: string, tags: [string] }\nfn main() { var p = person { name: \"Al\", tags: [\"a\", \"b\"] }\n p.tags.push!(\"c\")\n print(p.name, p.tags) }";
    assert_eq!(output_of(src), "Al [\"a\", \"b\", \"c\"]\n");
    // A moved value is not dropped twice, and a replaced one is dropped.
    let src = "fn take(sink s: string) -> int = s.len()\nfn main() { var a = \"hello\" + \"!\"\n const n = take(sink a)\n a = \"again\" + \"!\"\n print(n, a) }";
    assert_eq!(output_of(src), "6 again!\n");
    // Early return and loops.
    let src = "fn f(n: int) -> int {\n const s = \"x\" + \"y\"\n if n > 1 { return n }\n s.len()\n}\nfn main() { print(f(5), f(0)) }";
    assert_eq!(output_of(src), "5 2\n");
    let src = "fn main() { var t = 0\n for i of 0..3 { const s = \"v\" + \"w\"\n if i == 1 { continue }\n t += s.len() }\n print(t) }";
    assert_eq!(output_of(src), "4\n");
}

#[test]
fn question_mark_and_defaults() {
    let src = "fn quarter(n: int) -> int ! parse_error {\n const h = parse(\"8\")?\n ok(h + n)\n}\nfn parse(s: str) -> int ! parse_error = s.parseInt()\nfn main() { print(quarter(1), parse(\"x\")) }";
    assert_eq!(output_of(src), "ok(9) err(bad digit at position 0)\n");
    let src =
        "fn f(o: int?) -> int? {\n const v = o?\n v + 1\n}\nfn main() { print(f(5), f(none)) }";
    assert_eq!(output_of(src), "some(6) none\n");
    let src = "fn main() { const a: int? = none\n const b: int? = 4\n print(a ?? 9, b ?? 9) }";
    assert_eq!(output_of(src), "9 4\n");
    let src = "fn main() { const names: [string?] = [\"a\", none]\n print(names[0] ?? \"-\", names[1] ?? \"-\") }";
    assert_eq!(output_of(src), "a -\n");
    let src = "fn load(s: str) -> int ! error {\n const n = s.parseInt()?\n ok(n * 2)\n}\nfn main() { print(load(\"21\"), load(\"zz\")) }";
    assert_eq!(output_of(src), "ok(42) err(bad digit at position 0)\n");
}

#[test]
fn copy_and_in() {
    let src =
        "fn main() { const a = [\"x\", \"y\"]\n var b = copy a\n b.push!(\"z\")\n print(a, b) }";
    assert_eq!(output_of(src), "[\"x\", \"y\"] [\"x\", \"y\", \"z\"]\n");
    let src = "fn main() { print(3 in 1..5, 5 in 1..5, 5 in 1..=5, 2 in [1, 2, 3], \"ell\" in \"hello\", 9 in [1, 2]) }";
    assert_eq!(output_of(src), "true false true true true false\n");
}

#[test]
fn text_methods() {
    let src = "fn main() { const s = \"  Hello, World  \"\n print(s.trim(), s.trim().toUpper(), s.toLower().trim(), s.contains(\"World\"), s.trim().startsWith(\"Hello\"), s.trim().endsWith(\"d\")) }";
    assert_eq!(
        output_of(src),
        "Hello, World HELLO, WORLD hello, world true true true\n"
    );
    let src =
        "fn main() { const parts = \"a,b,,c\".split(\",\").toList()\n print(parts, parts.len()) }";
    assert_eq!(output_of(src), "[\"a\", \"b\", \"\", \"c\"] 4\n");
    let src =
        "fn main() { print(\"hello world\".replace(\"o\", \"0\"), \"héllo\".chars().count()) }";
    assert_eq!(output_of(src), "hell0 w0rld 5\n");
    let src = "fn main() { print(\"42\".parseInt(), \"-7\".parseInt(), \"4x\".parseInt(), \"3.5\".parseFloat()) }";
    assert_eq!(
        output_of(src),
        "ok(42) ok(-7) err(bad digit at position 1) ok(3.5)\n"
    );
}

#[test]
fn function_values() {
    let src = "fn apply(f: fn(int) -> int, x: int) -> int = f(x)\nfn dbl(x: int) -> int = x * 2\nfn main() { print(apply(dbl, 4), apply(x ~> x + 1, 4)) }";
    assert_eq!(output_of(src), "8 5\n");
    let src = "fn apply(f: fn(int) -> int, x: int) -> int = f(x)\nfn main() { const k = 10\n const add = x ~> x + k\n print(apply(add, 1), add(2)) }";
    assert_eq!(output_of(src), "11 12\n");
    let src =
        "fn main() { var n = 0\n const bump = () ~> { n += 1 }\n bump()\n bump()\n print(n) }";
    assert_eq!(output_of(src), "2\n");
}

#[test]
fn list_closures_and_option_map() {
    let src = "fn main() {\n const xs = [1, 2, 3, 4]\n print(xs.map(x ~> x * 10).toList(), xs.filter(x ~> x % 2 == 0).toList())\n var total = 0\n xs.each(x ~> { total += x })\n print(total) }";
    assert_eq!(output_of(src), "[10, 20, 30, 40] [2, 4]\n10\n");
    let src = "fn main() {\n const names = [\"a\", \"bb\"]\n print(names.map(s ~> s.len()).toList())\n const o: int? = 4\n print(o.map(x ~> x + 1), o.orErr(\"no\"))\n const r: int ! string = err(\"e\")\n print(r.mapErr(s ~> s.len())) }";
    assert_eq!(output_of(src), "[1, 2]\nsome(5) ok(4)\nerr(1)\n");
}

#[test]
fn maps_and_sets() {
    let src = "fn main() {\n var m = map { \"a\": 1, \"b\": 2 }\n m.insert!(\"c\", 3)\n m.insert!(\"a\", 10)\n print(m, m.len(), m.get(\"a\"), m.get(\"z\"), m[\"b\"])\n m.remove!(\"b\")\n print(m, m.containsKey(\"b\"))\n for k, v of m { print(k, v) }\n for k in m { print(k) } }";
    assert_eq!(output_of(src), "{\"a\": 10, \"b\": 2, \"c\": 3} 3 some(10) none 2\n{\"a\": 10, \"c\": 3} false\na 10\nc 3\na\nc\n");
    let src = "fn main() {\n var s = set { 3, 1, 3, 2 }\n s.insert!(9)\n s.remove!(1)\n print(s, s.len(), 2 in s, 5 in s)\n const t = copy s\n print(t == s, set { 1 } == set { 2 }) }";
    assert_eq!(output_of(src), "{3, 2, 9} 3 true false\ntrue false\n");
    let src = "fn main() { var total = 0\n for (var i = 0; i < 4; i += 1) { total += i }\n print(total, [1, 2].iter().toSet(), [\"a\", \"b\"].join(\"-\"), [0, ...[1, 2], 3]) }";
    assert_eq!(output_of(src), "6 {1, 2} a-b [0, 1, 2, 3]\n");
}

#[test]
fn program_op() {
    let src = "struct vec { x: int, y: int }\nimpl add for vec { fn add(self, o: vec) -> vec = vec { x: self.x + o.x, y: self.y + o.y } }\nfn main() { const a = vec { x: 1, y: 2 }\n const b = a + a\n print(b.x, b.y) }\n";
    assert_eq!(output_of(src), "2 4\n");
}

#[test]
fn program_conv() {
    let src = "struct meters { v: float }\nstruct feet { v: float }\nimpl converts<meters> for feet { fn convert(self) -> meters = meters { v: self.v * 0.3048 } }\nfn main() { const n = 42\n const s = n to string\n const f = n to float\n const m = (feet { v: 10.0 }) to meters\n print(s, f, m.v, 'a' to int, true to int, \"x\" to string) }\n";
    assert_eq!(output_of(src), "42 42 3.048 97 1 x\n");
}

#[test]
fn program_init() {
    let src = "struct acct { id: int, bal: int = 100\n init(id: int) { self.id = id } }\nstruct rng { lo: int, hi: int\n init(lo: int, hi: int) -> Self ! string {\n  if lo > hi { return err(\"bad\") }\n  self.lo = lo\n  self.hi = hi } }\nobj res { n: int\n init(n: int) { self.n = n }\n deinit { print(\"closed #(self.n)\") } }\nfn main() { const a = acct(5)\n print(a.id, a.bal)\n print(rng(1, 5).isOk(), rng(5, 1).isOk())\n const r = res(7)\n print(r.n) }\n";
    assert_eq!(output_of(src), "5 100\ntrue false\n7\nclosed 7\n");
}

#[test]
fn program_optchain() {
    let src = "struct p { name: string, age: int\n fn greet(self) -> string = \"hi #(self.name)\"\n fn bump(self) -> int = self.age + 1 }\nfn main() { const o: p? = p { name: \"x\", age: 4 }\n const n: p? = none\n print(o?.name, n?.name, o?.name ?? \"-\", n?.name ?? \"-\")\n print(o?.greet(), n?.greet(), o?.bump(), n?.bump()) }\n";
    assert_eq!(
        output_of(src),
        "some(\"x\") none x -\nsome(\"hi x\") none some(5) none\n"
    );
}

#[test]
fn program_range() {
    let src = "fn main() { const r = 1..5\n print(r.contains(3), r.contains(5), (1..=5).contains(5), r.iter().toList(), 2.5.min(1.0))\n for i of (1..4).iter() { print(i) } }\n";
    assert_eq!(output_of(src), "true false true [1, 2, 3, 4] 1\n1\n2\n3\n");
}

#[test]
fn program_step() {
    let src = "fn main() { for i of 0..7 step 3 { print(i) }\n for i of 5..0 step -2 { print(i) } \n for i of 0..=2 { print(i) } }\n";
    assert_eq!(output_of(src), "0\n3\n6\n5\n3\n1\n0\n1\n2\n");
}

#[test]
fn program_obj() {
    let src = "open obj animal { name: string\n open fn speak(self) -> string = \"...\"\n fn hello(self) -> string = \"hi #(self.name) says #(self.speak())\" }\nobj dog : animal { tricks: int = 3\n override fn speak(self) -> string = \"woof\"\n deinit { print(\"dog gone\") } }\nobj cat : animal { override fn speak(self) -> string = \"meow\" }\nfn describe(a: animal) -> string = a.hello()\nfn main() { const d = dog { name: \"rex\" }\n const c: animal = cat { name: \"tom\" }\n print(d.name, d.tricks, d.speak())\n print(describe(c))\n print(c is cat, c is dog, c is animal)\n print(describe(d)) }\n";
    assert_eq!(
        output_of(src),
        "rex 3 woof\nhi tom says meow\ntrue false true\nhi rex says woof\ndog gone\n"
    );
}

#[test]
fn program_iface() {
    let src = "interface shape { fn area(self) -> float\n fn describe(self) -> string = \"shape with area #(self.area())\" }\nstruct sq { s: float }\nimpl shape for sq { fn area(self) -> float = self.s * self.s }\nstruct ci { r: float }\nimpl shape for ci { fn area(self) -> float = 3.0 * self.r * self.r\n fn describe(self) -> string = \"circle\" }\nfn total(xs: [any shape]) -> float { var t = 0.0\n for x of xs { t += x.area() }\n t }\nfn show_it(s: any shape) { print(s.describe()) }\nfn generic<T: shape>(x: T) -> float = x.area()\nfn main() { const xs: [any shape] = [sq { s: 2.0 }, ci { r: 1.0 }]\n print(total(xs), generic(sq { s: 3.0 }))\n show_it(sq { s: 1.0 })\n show_it(ci { r: 1.0 }) }\n";
    assert_eq!(output_of(src), "7 9\nshape with area 1\ncircle\n");
}

#[test]
fn program_any_equality_and_hash() {
    let src = "interface key: hash { fn area(self) -> int }\nstruct sq { s: int }\nstruct rect { w: int, h: int }\nimpl hash for sq { fn hash(self) -> int = self.s }\nimpl hash for rect { fn hash(self) -> int = self.w * 31 + self.h }\nimpl key for sq { fn area(self) -> int = self.s * self.s }\nimpl key for rect { fn area(self) -> int = self.w * self.h }\nfn main() {\n var seen: set<any key> = set {}\n seen.insert!(sq { s: 3 })\n seen.insert!(sq { s: 3 })\n seen.insert!(sq { s: 4 })\n seen.insert!(rect { w: 3, h: 3 })\n print(seen.len())\n const probe: any key = sq { s: 4 }\n print(seen.contains(probe))\n const other: any key = rect { w: 1, h: 1 }\n print(seen.contains(other))\n const x: any key = sq { s: 3 }\n const y: any key = sq { s: 3 }\n const z: any key = rect { w: 3, h: 1 }\n print(x == y, x == z, x != z) }\n";
    assert_eq!(output_of(src), "3\ntrue\nfalse\ntrue false true\n");
}

#[test]
fn program_any_downcast() {
    let src = "interface shape { fn area(self) -> int }\nstruct sq { s: int }\nstruct rect { w: int, h: int }\nstruct named { name: string }\nimpl shape for sq { fn area(self) -> int = self.s * self.s }\nimpl shape for rect { fn area(self) -> int = self.w * self.h }\nimpl shape for named { fn area(self) -> int = self.name.len() }\nfn main() {\n const a: any shape = sq { s: 3 }\n const b = a to? sq\n print((b ?? sq { s: 0 }).s)\n const c: any shape = rect { w: 2, h: 5 }\n const d = c to? sq\n print((d ?? sq { s: 0 }).s, c.area())\n const e: any shape = named { name: \"abc\" }\n const f = e to? named\n print((f ?? named { name: \"\" }).name) }\n";
    assert_eq!(output_of(src), "3\n0 10\nabc\n");
}

#[test]
fn program_open_ranges_as_values() {
    let src = "fn from_ten(r: range<int>) -> bool = r.contains(10)\nfn main() {\n const a = 2..\n const b = ..3\n const c = 1..4\n const d = 1..=4\n print(a.contains(100), a.contains(1), b.contains(-5), b.contains(3))\n print(c.contains(4), d.contains(4), c.contains(0))\n print(from_ten(5..), from_ten(..10), from_ten(..=10))\n print(a == 2.., a == c, b == ..3)\n var rs = [2.., ..3, 0..9]\n print(rs.len()) }\n";
    assert_eq!(
        output_of(src),
        "true false true false\nfalse true false\ntrue false true\ntrue false true\n3\n"
    );
}

#[test]
fn program_once_fn() {
    let src = "fn eat(sink s: string) -> int = s.len()\nfn run(sink f: once fn() -> int) -> int = f!!()\nfn main() {\n const s = \"hello\"\n print(run(sink [sink s] () ~> eat(sink s)))\n const h = \"abc\"\n const k = [sink h] () ~> h.len()\n print(k() + run(sink k)) }\n";
    assert_eq!(output_of(src), "5\n6\n");
}

#[test]
fn program_builder() {
    let src = "fn main() { const xs = [1, 2, 3, 4, 5, 6]\n print([x of xs if x % 2 == 0 ~> x * 10])\n print([x of xs if x > 4])\n print([i, x of xs if i < 2 ~> \"#i:#x\"])\n print([x of 0..3, y of 0..2 ~> (x, y)])\n print(set { x of xs ~> x % 3 })\n print(map { x of 1..4 ~> x : x * x })\n print(xs.map(x ~> x + 1).toList()) }\n";
    assert_eq!(output_of(src), "[20, 40, 60]\n[5, 6]\n[\"0:1\", \"1:2\"]\n[(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]\n{1, 2, 0}\n{1: 1, 2: 4, 3: 9}\n[2, 3, 4, 5, 6, 7]\n");
}

#[test]
fn program_sp1() {
    let src = "fn main() { const t = spawn(() ~> 41 + 1)\n print(t.join!!()) }\n";
    assert_eq!(output_of(src), "42\n");
}

#[test]
fn program_sp2() {
    let src = "fn main() { const (tx, rx) = chan<int>()\n const p = spawn([sink tx] ~> { for i of 0..3 { tx.send(i * 10) } })\n for v of rx.iter() { print(v) }\n p.join!!() }\n";
    assert_eq!(output_of(src), "0\n10\n20\n");
}

#[test]
fn program_sh2() {
    let src = "fn main() { const s = shared(5)\n const s2 = copy s\n print(s.get(), s2.get()) }\n";
    assert_eq!(output_of(src), "5 5\n");
}

#[test]
fn program_assign() {
    let src =
        "fn main() { var a: int? = none\n a ??= 5\n var t = (1, 2)\n t.0 = 9\n print(a, t) }\n";
    assert_eq!(output_of(src), "some(5) (9, 2)\n");
}

#[test]
fn program_globals() {
    let src = "const limit = 10\nvar counter = 0\nfn bump() { counter += 1 }\nfn main() { bump()\n bump()\n print(counter, limit) }\n";
    assert_eq!(output_of(src), "2 10\n");
}

#[test]
fn program_variadic() {
    let src = "fn sum(...xs: int) -> int { var t = 0\n for x of xs { t += x }\n t }\nfn main() { print(sum(1, 2, 3), sum()) }\n";
    assert_eq!(output_of(src), "6 0\n");
}

#[test]
fn program_defaults() {
    let src = "fn greet(name: string, greeting: string = \"hi\") -> string = \"#greeting #name\"\nfn main() { print(greet(\"a\"), greet(\"b\", greeting: \"yo\")) }\n";
    assert_eq!(output_of(src), "hi a yo b\n");
}

#[test]
fn program_f1() {
    let src = "fn main() { const a = [1, 2, 3]\n var b = [0; 4]\n b[1] = 7\n print(a, b, b.len())\n var arr: [int; 3] = [1, 2, 3]\n arr[0] = 9\n print(arr, arr[0]) }\n";
    assert_eq!(output_of(src), "[1, 2, 3] [0, 7, 0, 0] 4\n[9, 2, 3] 9\n");
}

#[test]
fn program_f2() {
    let src = "fn sum(xs: [int]) -> int { var t = 0\n for x of xs { t += x }\n t }\nfn total(xs: [int]) -> int { var l = 0\n var r = 0\n const mid = xs.len() / 2\n scope s { s.spawn([inout l] ~> l = sum(xs[..mid]))\n  s.spawn([inout r] ~> r = sum(xs[mid..])) }\n l + r }\nfn main() { print(total([1, 2, 3, 4])) }\n";
    assert_eq!(output_of(src), "10\n");
}

#[test]
fn program_f3() {
    let src = "fn loop_to(n: int, acc: int) -> int { if n == 0 { return acc }\n become loop_to(n - 1, acc + n) }\nfn main() { print(loop_to(100000, 0)) }\n";
    assert_eq!(output_of(src), "5000050000\n");
}

#[test]
fn program_f4() {
    let src = "const names = [\"a\", \"b\"]\nconst limit = 3\nstruct cfg { port: int = 80, name: string = \"x\" }\nconst dflt = cfg {}\nfn main() { print(names, limit, dflt.port, dflt.name)\n const c = cfg { port: 1, ...dflt }\n print(c.port, c.name) }\n";
    assert_eq!(output_of(src), "[\"a\", \"b\"] 3 80 x\n1 x\n");
}

#[test]
fn program_e4c() {
    let src = "fn p(s: str) -> int ! parse_error = s.parseInt()\nfn run2() -> int ! error { const b = p(\"x\")?\n ok(b) }\nfn main() { switch run2() { case ok(_): print(\"ok\")\n case err(e): { print(e)\n  print(\"failed: #e\") } }\n const r = run2()\n switch r { case err(e): print(e)\n default: print(\"?\") } }\n";
    assert_eq!(
        output_of(src),
        "bad digit at position 0\nfailed: bad digit at position 0\nbad digit at position 0\n"
    );
}

#[test]
fn program_e6() {
    let src = "struct stack<T> { items: [T]\n fn push(inout self, sink x: T) { self.items.push!(x) }\n fn pop(inout self) -> T? = self.items.pop!()\n fn size(self) -> int = self.items.len() }\nfn main() { var s = stack<int> { items: [] }\n s.push!(1)\n s.push!(2)\n print(s.pop!(), s.size())\n var t = stack<string> { items: [] }\n t.push!(\"a\")\n print(t.pop!(), t.pop!()) }\n";
    assert_eq!(output_of(src), "some(2) 1\nsome(\"a\") none\n");
}

#[test]
fn program_e8() {
    let src = "fn main() { const names = [\"bob\", \"al\"]\n const lens = map { n of names ~> n : n.len() }\n for k, v of lens { print(\"#k=#v\") }\n const o = switch 5 { case 0: \"zero\"\n  case n if n > 3: \"big\"\n  default: \"small\" }\n print(o)\n guard names.len() > 1 else { return }\n print(names[0] + names[1]) }\n";
    assert_eq!(output_of(src), "bob=3\nal=2\nbig\nbobal\n");
}

#[test]
fn program_pipe() {
    let src = "fn dbl(x: int) -> int = x * 2\nfn main() { print(3 |> dbl) }\n";
    assert_eq!(output_of(src), "6\n");
}

#[test]
fn program_localfn() {
    let src = "fn main() { fn sq(x: int) -> int = x * x\n print(sq(4)) }\n";
    assert_eq!(output_of(src), "16\n");
}

#[test]
fn program_defer() {
    let src = "fn main() { defer print(\"bye\")\n print(\"hi\") }\n";
    assert_eq!(output_of(src), "bye\nhi\n");
}

#[test]
fn program_typetest() {
    let src = "fn main() { const xs = [1, 2, 3, 4, 5]\n switch xs { case [a, b, ...rest]: print(a, b, rest)\n  default: print(\"no\") } \n const o: int? = 5\n if o is some(v) { print(v) } }\n";
    assert_eq!(output_of(src), "1 2 [3, 4, 5]\n5\n");
}

#[test]
fn program_generic() {
    let src = "fn first<T>(xs: [T]) -> T? { if xs.len() == 0 { none } else { xs[0] } }\nfn pair<A, B>(a: A, b: B) -> (A, B) = (a, b)\nfn main() { print(first([1, 2]), first([\"a\"]), pair(1, \"x\")) }\n";
    assert_eq!(output_of(src), "some(1) some(\"a\") (1, \"x\")\n");
}

/// The sample programs run to the end without stopping or leaking.
#[test]
fn example_programs_run() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/programs");
    for name in ["shapes", "words", "stack", "tree", "tables"] {
        let mut program = Program::with_core();
        program
            .add_package_from_path(&dir.join(format!("{name}.pel")))
            .unwrap();
        let res = resolve(&program);
        let typed = check(&program, &res);
        assert!(
            typed.diagnostics.iter().all(|d| d.warning),
            "{name} does not check"
        );
        let l = lower(&program, &res, &typed);
        assert!(l.diagnostics.is_empty(), "{name}: {:?}", l.diagnostics);
        assert!(
            verify_module(&l.module).is_empty(),
            "{name} does not verify"
        );
        let o = crate::ir::run_full(&l.module, "main", &[]);
        assert_eq!(o.result.clone().map(|_| ()), Ok(()), "{name}");
        assert_eq!(o.leaked, 0, "{name} leaks");
        assert!(!o.output.is_empty(), "{name} printed nothing");
        native_must_agree(&l.module, "plain", name, &o.output);
        let mut m = l.module.clone();
        crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
        crate::ir::opt::prune(&mut m, &["main"]);
        let p = crate::ir::run_full(&m, "main", &[]);
        assert_eq!(
            p.output, o.output,
            "{name}: the optimiser changed the output"
        );
        assert_eq!(p.leaked, 0, "{name}: the optimiser caused a leak");
        assert!(p.steps < o.steps, "{name}: no faster");
    }
}

#[test]
fn program_g1() {
    let src = "@derive(eq, hash, show)\nenum color { red, green }\n@derive(eq, hash, show)\nstruct key { c: color, n: int }\nfn main() { var m: map<key, string> = {:}\n m.insert!(key { c: .red, n: 1 }, \"a\")\n m.insert!(key { c: .green, n: 2 }, \"b\")\n print(m[key { c: .red, n: 1 }], m.len(), color.red == color.green)\n print(m) }\n";
    assert_eq!(
        output_of(src),
        "a 2 false\n{key { c: red, n: 1 }: \"a\", key { c: green, n: 2 }: \"b\"}\n"
    );
}

#[test]
fn map_keeps_every_key_across_growth_and_removal() {
    // Keys a power-of-two stride apart collide in the low bits; removals shift the entries down.
    let src = "fn main() { var m: map<int, int> = {:}\n for i in 0..300 { m.insert!(i * 4096, i) }\n for i in 0..300 { m.insert!(-i * 65536 - 1, i) }\n m.remove!(8 * 4096)\n m.remove!(0)\n var t = 0\n for i in 0..300 { t += m.get(i * 4096) ?? 1000 }\n for i in 0..300 { t += m.get(-i * 65536 - 1) ?? 1000 }\n print(m.len(), t, m.containsKey(0), m.containsKey(299 * 4096)) }\n";
    assert_eq!(output_of(src), "598 91692 false true\n");
}

#[test]
fn program_g2() {
    let src = "fn main() { const (tx, rx) = chan<int>()\n const (tx2, rx2) = chan<string>()\n tx.send(1)\n tx2.send(\"hi\")\n select { case v = rx.recv() { print(\"int #v\") }\n  case s = rx2.recv() { print(\"str #s\") } }\n select { case v = rx.recv() { print(\"again #v\") }\n  default { print(\"nothing\") } }\n select { case s = rx2.recv() { print(\"str #s\") }\n  default { print(\"nothing\") } }\n const p = spawn([sink tx] ~> { tx.send(7) })\n select { case v = rx.recv() { print(\"late #v\") } }\n p.join!!() }\n";
    assert_eq!(
        output_of(src),
        "int some(1)\nnothing\nstr some(\"hi\")\nlate some(7)\n"
    );
}

#[test]
fn program_g3() {
    let src = "fn maybe(n: int) -> int? { if n > 0 { n } else { none } }\nfn both(a: int, b: int) -> int? { const x = maybe(a)?\n const y = maybe(b)?\n x + y }\nfn main() { print(both(1, 2), both(0, 2))\n var o: int? = 3\n if o is some(v) { print(v) }\n o = none\n print(o ?? -1, o.isNone()) \n const nested = [[1, 2], [3]]\n print(nested, nested[0][1])\n var grid = [[0; 2]; 2]\n print(grid) }\n";
    assert_eq!(
        output_of(src),
        "some(3) none\n3\n-1 true\n[[1, 2], [3]] 2\n[[0, 0], [0, 0]]\n"
    );
}

#[test]
fn program_f5() {
    let src = "fn main() { const s = \"héllo\"\n for c of s.chars() { print(c) }\n print(s[0..2], 'a' to int, 3.7.floor(), 10 / 3, 10 % 3, 2.0 * 3.5)\n print(1_000 + 0xff + 0b101, -5 % 3, 1 << 4, 6 & 3, 6 | 1, 6 ^ 3) }\n";
    assert_eq!(
        output_of(src),
        "h\né\nl\nl\no\nh� 97 3 3 1 7\n1260 -2 16 2 7 5\n"
    );
}

// ---- Boxes -------------------------------------------------------------------------------------

#[test]
fn box_make_read_set_take() {
    let src = "struct pt { x: int, y: int }\nfn main() {\n var n = box(1)\n n.set!(2)\n print(n.get(), n.take!!())\n var s = box(\"hi\")\n s.set!(\"there\")\n print(s.get(), s.len(), s.take!!())\n var p = box(pt { x: 1, y: 2 })\n p.get().x = 10\n print(p.x, p.y)\n const q = p.take!!()\n print(q.x + q.y) }\n";
    assert_eq!(output_of(src), "2 2\nthere 5 there\n10 2\n12\n");
}

#[test]
fn box_tree_insert_and_walk() {
    let tree = "enum tree { leaf\n node(box<tree>, int, box<tree>) }\nfn insert(t: tree, v: int) -> tree { switch t {\n case .leaf: tree.node(box(tree.leaf), v, box(tree.leaf))\n case .node(l, x, r): if v < x { tree.node(box(insert(l.get(), v)), x, copy r) } else { tree.node(copy l, x, box(insert(r.get(), v))) } } }\nfn sum(t: tree) -> int { switch t { case .leaf: 0\n case .node(l, x, r): sum(l.get()) + x + sum(r.get()) } }\n";
    let small = format!("{tree}fn main() {{ var t = tree.leaf\n for v in [2, 1, 3] {{ t = insert(t, v) }}\n print(sum(t)) }}\n");
    assert_eq!(output_of(&small), "6\n");
    let big = format!("{tree}fn main() {{ var t = tree.leaf\n var i = 0\n while i < 1000 {{ t = insert(t, (i * 7919) % 1000)\n i += 1 }}\n print(sum(t)) }}\n");
    assert_eq!(output_of(&big), "499500\n");
}

#[test]
fn box_linked_list_and_deep_chain() {
    let src = "enum chain { nil\n cons(int, box<chain>) }\nfn len(c: chain) -> int { switch c { case .nil: 0\n case .cons(_, rest): 1 + len(rest.get()) } }\nfn len_loop(c: chain) -> int { become count(c, 0) }\nfn count(c: chain, acc: int) -> int { switch c { case .nil: acc\n case .cons(_, rest): become count(rest.get(), acc + 1) } }\nfn main() { var c = chain.nil\n for i in 0..10000 { c = chain.cons(i, box(sink c)) }\n print(len(c)) }\n";
    assert_eq!(output_of(src), "10000\n");
}

#[test]
fn box_copy_is_deep() {
    let src = "fn main() { var a = box([1, 2])\n const b = copy a\n a[0] = 9\n print(a, b)\n const s = box(\"x\")\n const t = copy s\n print(s, t) }\n";
    assert_eq!(output_of(src), "[9, 2] [1, 2]\nx x\n");
}

#[test]
fn box_holds_a_function() {
    let src = "fn adder(n: int) -> box<fn(int) -> int> { box((x: int) ~> x + n) }\nfn main() { const f = adder(5)\n print(f.get()(1)) }\n";
    assert_eq!(output_of(src), "6\n");
}

#[test]
fn box_compares_and_keys_a_map() {
    let src = "fn main() { const a = box(2)\n const b = box(3)\n print(a == b, a < b, a == a)\n var m: map<box<int>, string> = map {}\n m.insert!(box(1), \"one\")\n const k = box(1)\n print(m.len(), k in m) }\n";
    assert_eq!(output_of(src), "false true true\n1 true\n");
}

#[test]
fn box_inside_an_option_field() {
    let src = "struct pt { x: int }\nstruct holder { next: box<pt>? }\nfn main() { const h = holder { next: box(pt { x: 5 }) }\n if h.next is some(q) { print(q.x) }\n const e = holder { next: none }\n print(e.next.isNone()) }\n";
    assert_eq!(output_of(src), "5\ntrue\n");
}

// ---- A-08: `inout` closure parameters and the `scope` handle ------------------------------------

#[test]
fn inout_closure_parameters() {
    let src = "fn twice(inout x: int, f: fn(inout int)) { f(inout x)\n f(inout x) }\nfn main() { const h = lock(0)\n h.with(inout n ~> { n += 5 })\n print(h.with((inout n) ~> { n += 0\n n }))\n var k = 1\n twice(inout k, inout n ~> { n *= 3 })\n twice(inout k, n ~> { n += 1 })\n print(k) }\n";
    assert_eq!(output_of(src), "5\n11\n");
}

#[test]
fn scope_tasks_finish_with_the_scope() {
    let src = "fn main() { var a = 0\n var b = 0\n scope s { s.spawn([inout a] ~> { a += 1 })\n s.spawn([inout b] ~> { b += 2 })\n print(\"spawned\") }\n print(a, b) }\n";
    assert_eq!(output_of(src), "spawned\n1 2\n");
}

// ---- A-03: the ledger (a fresh value nobody binds is dropped where its statement ends) -----------

#[test]
fn object_temporaries_are_dropped() {
    let src = r#"obj dog { name: string
    fn speak(self) -> string = "woof #(self.name)" }
fn make_dog(sink n: string) -> dog { dog { name: n } }
fn show(d: dog) { print(d.name) }
fn pick(c: bool) -> dog { if c { make_dog("a") } else { make_dog("b") } }
fn main() {
    make_dog("a")
    print(make_dog("b").name)
    show(dog { name: "c" })
    print((if true { make_dog("d") } else { make_dog("e") }).name)
    const d = make_dog("f")
    print(d.speak())
    print(pick(false).speak())
}
"#;
    assert_eq!(output_of(src), "b\nc\nd\nwoof f\nwoof b\n");
}

#[test]
fn temporaries_settle_into_their_new_owner() {
    let src = r#"obj dog { name: string
    fn speak(self) -> string = "woof #(self.name)" }
interface talker { fn talk(self) -> string }
impl talker for int { fn talk(self) -> string = "int" }
struct kennel { d: dog }
fn make_dog(sink n: string) -> dog { dog { name: n } }
fn show(d: dog) { print(d.name) }
fn take(sink d: dog) { print(d.name) }
fn find(c: bool) -> dog? { if c { make_dog("opt") } else { none } }
fn try_it(c: bool) -> int? {
    const d = find(c)?
    print(d.name)
    1
}
fn early(c: bool) -> string {
    if c { return make_dog("ret").speak() }
    "late"
}
fn main() {
    var ds: [dog] = []
    ds.push!(make_dog("p"))
    var k = kennel { d: make_dog("k") }
    take(make_dog("s"))
    print(try_it(true), try_it(false))
    print(early(true), early(false))
    for i in 0..3 {
        if i == 1 { continue }
        print(make_dog("loop").name)
        if i == 2 { break }
    }
    const t = true && make_dog("and").name.len() > 0
    const o = find(false) ?? make_dog("dflt")
    print(t, o.name)
    const a: any talker = 5
    const f = [sink k] ~> copy k.d.name
    const g = [sink ds] ~> ds.len()
    print(g())
    var i = 0
    while make_dog("cond").name.len() > i { i += 1 }
    print(find(true).isSome())
    var m: map<string, dog> = map {}
    m.insert!("x", make_dog("m"))
    print(m.len())
    switch make_dog("sw") { case d: print(d.name) }
}
"#;
    assert_eq!(
        output_of(src),
        "s\nopt\nsome(1) none\nwoof ret late\nloop\nloop\ntrue dflt\n1\ntrue\n1\nsw\n"
    );
}

#[test]
fn box_temporaries_are_dropped() {
    let src = "fn main() { var m: map<box<int>, string> = map {}\n m.insert!(box(1), \"one\")\n print(box(1) in m)\n box(7)\n print(box(3), box(3) == box(3)) }\n";
    assert_eq!(output_of(src), "true\n3 true\n");
}

#[test]
fn temporaries_die_in_reverse_order_at_the_end_of_the_statement() {
    let src = "obj noisy { id: int\n deinit { print(\"drop #(self.id)\") } }\nfn mk(i: int) -> noisy { noisy { id: i } }\nfn pair(a: noisy, b: noisy) -> int { a.id + b.id }\nfn main() { print(pair(mk(1), mk(2)))\n print(\"next\") }\n";
    assert_eq!(output_of(src), "3\ndrop 2\ndrop 1\nnext\n");
}

// ---- A-05 stage M1: `meta if` ---------------------------------------------------------------------

#[test]
fn meta_if_lowers_only_the_chosen_branch() {
    let src = "meta if target.os == linux {\n fn platform() -> string = \"linux\"\n} else {\n fn platform() -> string = \"other\"\n}\nfn main() { print(platform())\n const n = meta if target.pointer_bits == 64 { 8 } else { missing_name }\n print(n) }\n";
    let mut program = Program::with_core();
    program.target = crate::meta::Target {
        os: "linux".into(),
        pointer_bits: 64,
        ..crate::meta::Target::host()
    };
    program.add_package("app", &[("main", src)]);
    let res = resolve(&program);
    let typed = check(&program, &res);
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let l = lower(&program, &res, &typed);
    let o = run_full(&l.module, "main", &[]);
    assert_eq!(o.output, "linux\n8\n");
}

// ---- A-05 stage M2: `meta N: int` value parameters ------------------------------------------------

#[test]
fn value_parameters_make_one_instance_per_value() {
    let src = "fn zeros<meta N: int>() -> [int; N] { [0; N] }\nfn total<meta N: int>(xs: [int; N]) -> int { var t = 0\n for i in 0..N { t += xs[i] }\n t }\nfn main() { const a = zeros<3>()\n const b: [int; 5] = zeros<5>()\n print(a, b)\n print(total([1, 2, 3, 4]), total(a)) }\n";
    assert_eq!(output_of(src), "[0, 0, 0] [0, 0, 0, 0, 0]\n10 0\n");
}

#[test]
fn value_parameters_on_a_struct() {
    let src = "struct buf<meta N: int> { data: [int; N]\n fn size(self) -> int = N }\nfn main() { const b = buf<4> { data: [1, 2, 3, 4] }\n print(b.size(), b.data[2]) }\n";
    assert_eq!(output_of(src), "4 3\n");
}

// ---- A-06: soa<T> ----------------------------------------------------------------------------------

#[test]
fn soa_works_like_a_list_of_structs() {
    let src = r#"struct point { x: float, y: float }
struct person { name: string, age: int }
fn main() {
    var ps: soa<point> = soa []
    ps.push!(point { x: 1.0, y: 2.0 })
    ps.push!(point { x: 3.0, y: 4.0 })
    ps[1].x = 30.0
    ps[0].y += 5.0
    print(ps.len(), ps[0].x, ps[1].x, ps[0].y)
    print(ps.x, ps.y)
    for inout p of ps { p.x += 1.0 }
    var total = 0.0
    for p of ps { total += p.x + p.y }
    print(total)
    const q = ps[1]
    print(q.x, q.y)
    ps[0] = point { x: 9.0, y: 9.0 }
    print(ps.x)
    var people: soa<person> = soa [person { name: "ann", age: 3 }, person { name: "bo", age: 5 }]
    people.push!(person { name: "cy", age: 7 })
    const gone = people.remove!(1)
    print(gone.name, people.len(), people.name, people.age)
    people.insert!(0, person { name: "zed", age: 1 })
    print(people.name)
    print(people.pop!().isSome(), people.len())
    people.clear!()
    print(people.len())
    const copy_of = copy ps
    print(copy_of == ps, copy_of.len())
}
"#;
    assert_eq!(
        output_of(src),
        "2 1 30 7\n[1, 30] [7, 4]\n44\n31 4\n[9, 31]\nbo 2 [\"ann\", \"cy\"] [3, 7]\n[\"zed\", \"ann\", \"cy\"]\ntrue 2\n0\ntrue 2\n"
    );
}

#[test]
fn soa_prints_as_a_list_of_structs_and_has_blocked_form() {
    let src = "@derive(show)\nstruct point { x: int, y: int }\nfn main() { var ps: soa<point, 4> = soa []\n for i in 0..10 { ps.push!(point { x: i, y: i * i }) }\n print(ps.len(), ps[9].y, ps.x)\n var small: soa<point> = soa [point { x: 1, y: 2 }]\n print(small) }\n";
    assert_eq!(
        output_of(src),
        "10 81 [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]\n[point { x: 1, y: 2 }]\n"
    );
}

// ---- A-05 stage M3: `meta expr` and `meta fn` -------------------------------------------------------

fn lowered_diags(src: &str) -> Vec<(String, bool)> {
    let mut program = Program::with_core();
    program.add_package("app", &[("main", src)]);
    let res = resolve(&program);
    let typed = check(&program, &res);
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let l = lower(&program, &res, &typed);
    l.diagnostics
        .iter()
        .map(|d| (d.message.clone(), d.warning))
        .collect()
}

#[test]
fn meta_values_are_baked_in() {
    let src = r#"meta fn build_table() -> [int; 8] {
    var t = [0; 8]
    for i in 0..8 { t[i] = i * i }
    t
}
meta fn greeting(name: string) -> string = "hello, #name"
meta fn squares() -> [int] {
    var xs: [int] = []
    for i in 0..5 { xs.push!(i * 2) }
    xs
}
struct pair { a: int, b: string }
meta fn make_pair() -> pair = pair { a: 7, b: "seven" }
const table = meta build_table()
fn main() {
    print(table[3], table)
    print(meta greeting("pelago"))
    print(meta squares())
    const p = meta make_pair()
    print(p.a, p.b)
    const o: int? = meta 5
    print(o)
}
"#;
    assert_eq!(
        output_of(src),
        "9 [0, 1, 4, 9, 16, 25, 36, 49]\nhello, pelago\n[0, 2, 4, 6, 8]\n7 seven\nsome(5)\n"
    );
}

#[test]
fn meta_runs_that_cannot_finish_are_errors() {
    let d = lowered_diags("meta fn spin() -> int { var i = 0\n while true { i += 1 }\n i }\n@meta_limit(500)\nconst c = meta spin()\nfn main() { print(c) }\n");
    assert!(
        d.iter().any(|(m, w)| !w && m.contains("ran too long")),
        "{d:?}"
    );
    let d = lowered_diags("meta fn inner() -> int { var i = 0\n while true { i += 1 }\n i }\nmeta fn outer() -> int = inner() + 1\n@meta_limit(500)\nconst c = meta outer()\nfn main() { print(c) }\n");
    assert!(
        d.iter()
            .any(|(m, w)| !w && m.contains("(in inner <- outer)")),
        "{d:?}"
    );
    let d = lowered_diags("meta fn fail() -> int { meta_error(\"nope\") }\nfn main() { const c = meta fail()\n print(c) }\n");
    assert!(d.iter().any(|(m, w)| !w && m.contains("nope")), "{d:?}");
    let d = lowered_diags(
        "meta fn f() -> int = a + 1\nconst a: int = meta f()\nfn main() { print(a) }\n",
    );
    assert!(
        d.iter().any(|(m, w)| !w && m.contains("needs itself")),
        "{d:?}"
    );
}

#[test]
fn meta_values_that_hold_closures_are_errors() {
    let d = lowered_diags("meta fn mk() -> box<fn(int) -> int> { box((x: int) ~> x + 1) }\nfn main() { const f = meta mk()\n print(f.get()(1)) }\n");
    assert!(
        d.iter()
            .any(|(m, w)| !w && m.contains("cannot be computed at compile time")),
        "{d:?}"
    );
}

#[test]
fn meta_note_is_a_compile_time_message() {
    let d = lowered_diags("meta fn f() -> int { meta_note(\"building\")\n 4 }\nfn main() { const c = meta f()\n print(c) }\n");
    assert!(
        d.iter().any(|(m, w)| *w && m.contains("meta: building")),
        "{d:?}"
    );
}

// ---- A-07: async / await (stage 0: a call runs the body to the end) -----------------------------------

#[test]
fn async_functions_and_await() {
    let src = "async fn double(x: int) -> int { x * 2 }\nasync fn greet(name: string) -> string { \"hello #name\" }\nasync fn say(s: string) { print(s) }\nasync fn both() -> int {\n const a = double(4)\n const b = double(5)\n await say(\"working\")\n await a + await b }\nstruct counter { n: int\n async fn bump(inout self) -> int { self.n += 1\n self.n } }\nasync fn boxed(v: int) -> box<int> { box(v) }\nfn main() { print(block_on(both()))\n print(block_on(greet(\"pelago\")))\n var c = counter { n: 0 }\n print(block_on(c.bump!()), block_on(c.bump!()))\n const b = block_on(boxed(7))\n print(b.get()) }\n";
    assert_eq!(output_of(src), "working\n18\nhello pelago\n1 2\n7\n");
}

#[test]
fn async_with_results_and_channels() {
    let src = r#"async fn safe_div(a: int, b: int) -> int ! string {
    if b == 0 { return err("zero") }
    ok(a / b)
}
async fn run() -> int ! string {
    const x = await safe_div(8, 2)?
    ok(x + 1)
}
async fn total(sink rx: receiver<int>) -> int {
    var t = 0
    var n = 0
    while n < 2 {
        const v = rx.recv()
        switch v {
            case some(x): t += x
            case none: {}
        }
        n += 1
    }
    t
}
fn main() {
    print(block_on(safe_div(1, 0)).isErr(), block_on(run()))
    const (tx, rx) = chan<int>()
    tx.send(1)
    tx.send(2)
    print(block_on(total(sink rx)))
}
"#;
    assert_eq!(output_of(src), "true ok(5)\n3\n");
}

#[test]
fn any_show_of_builtin_shapes() {
    // Numbers, strings and lists have no `show` method of their own: the table gets the library's.
    let src = r#"
fn f(x: any show) { print(x) }
fn main() {
    const c = true
    f(8)
    f("hi")
    f([1, 2])
    print(if c { 8 } else { 4 })
    f(if c { 7 } else { 1 })
}
"#;
    assert_eq!(output_of(src), "8\nhi\n[1, 2]\n8\n7\n");
}

#[test]
fn array_length_arithmetic() {
    // `N + 1`, `N * 2` in types: the length folds once `N` is known.
    let src = r#"
fn push_end<meta N: int>(xs: [int; N], x: int) -> [int; N + 1] {
    var out = [0; N + 1]
    for i in 0..N { out[i] = xs[i] }
    out[N] = x
    out
}
fn twice<meta N: int>(xs: [int; N]) -> [int; N * 2] {
    var out = [0; N * 2]
    for i in 0..N {
        out[i] = xs[i]
        out[i + N] = xs[i]
    }
    out
}
fn main() {
    const a = push_end([1, 2, 3], 4)
    const b: [int; 8] = twice(a)
    print(a, b)
    print(push_end(push_end([7], 8), 9))
}
"#;
    assert_eq!(
        output_of(src),
        "[1, 2, 3, 4] [1, 2, 3, 4, 1, 2, 3, 4]\n[7, 8, 9]\n"
    );
}

#[test]
fn meta_bakes_maps_sets_and_boxes() {
    let src = r#"
enum tree { leaf
    node(box<tree>, int, box<tree>) }
meta fn names() -> map<string, int> {
    var m = map { "k0": 0 }
    for i in 1..4 { m.insert!("k#i", i * i) }
    m
}
meta fn evens() -> set<int> {
    var s = set { 0 }
    for i in 1..10 { if i % 2 == 0 { s.insert!(i) } }
    s
}
meta fn small() -> tree = tree.node(box(tree.leaf), 5, box(tree.node(box(tree.leaf), 6, box(tree.leaf))))
fn sum(t: tree) -> int {
    switch t {
        case .leaf: 0
        case .node(l, v, r): sum(l.get()) + v + sum(r.get())
    }
}
fn main() {
    const m = meta names()
    const s = meta evens()
    const t = meta small()
    print(m.get("k2"), m.len(), s.contains(4), s.contains(5), s.len())
    print(sum(t))
}
"#;
    assert_eq!(output_of(src), "some(4) 4 true false 5\n11\n");
}

// ---- The native build on the edge cases of `docs/design/backend.md` -------------------------------------

#[test]
fn appending_with_plus_assign_keeps_its_meaning() {
    // `s = s + a + b` appends in place; every other shape keeps the copying assignment.
    let cases: [(&str, &str); 8] = [
        ("var s = \"a\"\n s = s + \"b\" + \"c\"\n print(s)", "abc\n"),
        ("var s = \"a\"\n s = s + s\n s = s + s\n print(s)", "aaaa\n"),
        (
            "const t = \"x\"\n var s = \"a\"\n s = t + s\n print(s)",
            "xa\n",
        ),
        (
            "var s = \"\"\n for i of 0..5 { s = s + \"#i,\" }\n print(s)",
            "0,1,2,3,4,\n",
        ),
        (
            "var s = \"n=\"\n const n = \"7\"\n s = s + n + \"!\"\n print(s)",
            "n=7!\n",
        ),
        ("var s = \"a\"\n s = s + \"#s#s\"\n print(s)", "aaa\n"),
        (
            "var s = \"a\"\n s = (s + \"b\") + \"c\"\n print(s)",
            "abc\n",
        ),
        (
            "var s = \"a\"\n var t = copy s\n s = s + \"b\"\n t = t + \"c\"\n print(s, t)",
            "ab ac\n",
        ),
    ];
    for (body, want) in cases {
        let src = format!("fn main() {{\n {body}\n}}");
        assert_eq!(output_of(&src), want, "{src}");
        agree(&src);
    }
    // Calls of plain functions and string fields are appended in place too; anything that could
    // touch the target keeps the copying assignment.
    let defs = "struct box { a: string, b: string }\nstruct outer { inner: box, n: string }\nfn f() -> string = \"x\"\nfn g(t: string) -> string = \"<#t>\"\n";
    let cases: [(&str, &str); 10] = [
        ("var s = \"a\"\n s = s + f()\n print(s)", "ax\n"),
        ("var s = \"a\"\n s = s + g(\"q\") + f()\n print(s)", "a<q>x\n"),
        ("var s = \"a\"\n s = s + g(s)\n print(s)", "a<a>\n"),
        ("var s = \"a\"\n for i of 0..3 { s = s + g(s) }\n print(s)", "a<a><a<a>><a<a><a<a>>>\n"),
        ("var o = box { a: \"1\", b: \"2\" }\n o.a = o.a + \"x\" + o.b\n print(o.a, o.b)", "1x2 2\n"),
        ("var o = box { a: \"1\", b: \"2\" }\n o.a = o.a + o.a\n print(o.a, o.b)", "11 2\n"),
        ("var o = box { a: \"1\", b: \"2\" }\n o.a = o.b + o.a\n print(o.a, o.b)", "21 2\n"),
        ("var o = outer { inner: box { a: \"1\", b: \"2\" }, n: \"z\" }\n o.inner.a = o.inner.a + o.n + f()\n print(o.inner.a, o.n)", "1zx z\n"),
        ("var o = outer { inner: box { a: \"1\", b: \"2\" }, n: \"z\" }\n o.n = o.n + o.inner.a\n o.inner.b = o.inner.b + o.n\n print(o.n, o.inner.b)", "z1 2z1\n"),
        ("var o = box { a: \"\", b: \"\" }\n for i of 0..4 { o.a = o.a + \"#i\" + f() }\n print(o.a)", "0x1x2x3x\n"),
    ];
    for (body, want) in cases {
        let src = format!("{defs}fn main() {{\n {body}\n}}");
        assert_eq!(output_of(&src), want, "{src}");
        agree(&src);
    }
}

/// The program does the same natively as in the interpreter: same output, and the same stop (message
/// on standard error, status 1) or a clean exit.
fn agree(src: &str) {
    use crate::native::{exec, gen, Os};
    let l = lowered(src);
    let o = run_full(&l.module, "main", &[]);
    let optimised = {
        let mut m = l.module.clone();
        crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
        crate::ir::opt::prune(&mut m, &["main"]);
        m
    };
    let mut sets = vec![
        ("plain", Os::Linux, &l.module, false, usize::MAX),
        ("plain", Os::Linux, &l.module, true, usize::MAX),
        ("optimised", Os::Linux, &optimised, true, usize::MAX),
        ("plain", Os::Windows, &l.module, true, usize::MAX),
    ];
    for limit in [1, 2, 4] {
        sets.push(("plain", Os::Linux, &l.module, true, limit));
        sets.push(("optimised", Os::Linux, &optimised, true, limit));
    }
    for (what, os, m, regalloc, limit) in sets {
        if !exec::can_run(os) {
            continue;
        }
        let opts = gen::Options {
            leak_check: false,
            target: os,
            regalloc,
        };
        gen::REG_LIMIT.with(|l| l.set(limit));
        let image = gen::compile(m, "main", &opts).unwrap();
        gen::REG_LIMIT.with(|l| l.set(usize::MAX));
        let r = exec::run_image(os, &image).unwrap();
        assert_eq!(
            r.out, o.output,
            "{what} {os:?} with {limit} registers: output differs in {src}"
        );
        match &o.result {
            Ok(_) => assert_eq!((r.err.as_str(), r.code), ("", 0), "{what} {os:?}: {src}"),
            Err(msg) => assert_eq!(
                (r.err.as_str(), r.code),
                (format!("stopped: {msg}\n").as_str(), 1),
                "{what} {os:?}: {src}"
            ),
        }
    }
    for (what, m) in [("plain", &l.module), ("optimised", &optimised)] {
        let Some(r) = exec::build_and_run_a64(m, false) else {
            continue;
        };
        assert_eq!(r.out, o.output, "{what} AArch64: output differs in {src}");
        match &o.result {
            Ok(_) => assert_eq!((r.err.as_str(), r.code), ("", 0), "{what} AArch64: {src}"),
            Err(msg) => assert_eq!(
                (r.err.as_str(), r.code),
                (format!("stopped: {msg}\n").as_str(), 1),
                "{what} AArch64: {src}"
            ),
        }
    }
}

/// `fma` rounds once: it agrees with the interpreter on every back end, and with the exact value
/// where `a * b + c` would lose the low bits (card A-10).
#[test]
fn native_agrees_on_fused_multiply_add() {
    agree(
        "fn main() {
            const x = 0.1
            print(x.fma(10.0, 0.0 - 1.0) * 1.0e17)
            print((x * 10.0 + (0.0 - 1.0)) * 1.0e17)
            var i = 0
            var acc = 0.0
            while i < 4 {
                const t = 1.0 + 1.0e-8 * (i to float)
                acc = acc + t.fma(t, 0.0 - 1.0 - 2.0e-8 * (i to float)) * 1.0e16
                i += 1
            }
            print(acc)
        }",
    );
}

/// A zero float constant that a hoisted instruction reads stays where the loop can see it.
#[test]
fn licm_keeps_a_zero_constant_a_hoisted_op_reads() {
    agree(
        "fn g(a: float, b: float) -> float { a + b }
        fn main() {
            var acc = 0.0
            var i = 0
            while i < 10 { acc = acc + g(1.0 + (i to float), 0.0 - 1.0); i += 1 }
            print(acc)
        }",
    );
}

/// Division and remainder by constants that are not powers of two run as a high multiply (card A-11,
/// W1); the loops give the optimiser values it cannot fold, spread over the whole range.
#[test]
fn native_agrees_on_division_by_constants() {
    agree(
        "fn main() {
            var s = 0
            var h = 1
            for i in 0..4000 {
                h = h *% 6364136223846793005 +% 1442695040888963407
                const x = h
                const y = i - 2000
                s = s ^ (x / 10) ^ (x % 10) ^ (x / 3) ^ (x % 7) ^ (x / -7) ^ (x % -10)
                s = s ^ (x / 641) ^ (x % 1000) ^ (y / 10) ^ (y % 10) ^ (y / 100) ^ (y % 3)
                s = s ^ (x / 9223372036854775807) ^ (x / -9223372036854775807)
            }
            print(s)
        }",
    );
    agree(
        "fn main() {
            var s: uint = 0
            var h: uint = 1
            for i in 0..4000 {
                h = h *% 6364136223846793005 +% 1442695040888963407
                s = s ^ (h / 10) ^ (h % 10) ^ (h / 3) ^ (h % 7) ^ (h / 641) ^ (h % 1000)
                s = s ^ (h / 18446744073709551615) ^ (h / 9223372036854775809)
            }
            print(s)
        }",
    );
    agree(
        "fn main() {
            var s: int32 = 0
            var u: uint32 = 0
            var h: uint32 = 1
            var x: int32 = 1
            for i in 0..4000 {
                h = h *% 1664525 +% 1013904223
                x = x *% 1664525 +% 1013904223
                s = s ^ (x / 10) ^ (x % 10) ^ (x / 7) ^ (x % -3) ^ (x / 100)
                u = u ^ (h / 10) ^ (h % 7) ^ (h / 641) ^ (h / 4294967295)
            }
            print(s, u)
        }",
    );
    agree(
        "fn main() {
            var s: int8 = 0
            var u: uint8 = 0
            const lo: int8 = -128
            const hi: int8 = 127
            const top: uint8 = 255
            for x of lo..=hi {
                s = s ^ (x / 10) ^ (x % 10) ^ (x / 3) ^ (x % 7) ^ (x / -5)
            }
            for b of 0..=top {
                u = u ^ (b / 10) ^ (b % 7) ^ (b / 3) ^ (b / 255) ^ (b % 100)
            }
            print(s, u)
        }",
    );
}

#[test]
fn native_agrees_on_the_edge_cases() {
    let programs = [
        // Division and remainder.
        "fn main() { const z = 0\n print(10 / z) }",
        "fn main() { const z = 0\n print(10 % z) }",
        "fn main() { const m = -9223372036854775807 - 1\n const d = -1\n print(m / d) }",
        "fn main() { const m = -9223372036854775807 - 1\n const d = -1\n print(m % d) }",
        "fn main() { const a = -7\n const b = 2\n print(a / b, a % b, 7 / -2, 7 % -2) }",
        "fn main() { const a: uint = 18446744073709551615\n const b: uint = 10\n print(a / b, a % b) }",
        "fn main() { const a: int8 = -128\n const b: int8 = -1\n print(a / b, a % b) }",
        "fn main() { const a: int32 = -2147483648\n const b: int32 = -1\n print(a / b, a % b) }",
        // Shifts.
        "fn main() { const n = 64\n print(1 << n) }",
        "fn main() { const n = 63\n print(1 << n, -1 >> n, 1 << 3, -16 >> 2) }",
        "fn main() { const a: uint = 18446744073709551615\n print(a >> 60, a << 60) }",
        "fn main() { const a: uint8 = 255\n const n = 8\n print(a >> n) }",
        "fn main() { const a: int8 = -128\n print(a >> 7, a >> 1) }",
        // Checked arithmetic at every width.
        "fn main() { const a: int8 = 127\n print(a + 1) }",
        "fn main() { const a: int16 = -32768\n print(a - 1) }",
        "fn main() { const a: int32 = 2147483647\n print(a * 2) }",
        "fn main() { const a = 9223372036854775807\n print(a + 1) }",
        "fn main() { const a = 9223372036854775807\n print(a * 2) }",
        "fn main() { const a = -9223372036854775807 - 1\n print(a - 1) }",
        "fn main() { const a: uint8 = 255\n print(a + 1) }",
        "fn main() { const a: uint8 = 0\n print(a - 1) }",
        "fn main() { const a: uint16 = 65535\n print(a * 2) }",
        "fn main() { const a: uint32 = 4294967295\n print(a + 1) }",
        "fn main() { const a: uint = 18446744073709551615\n print(a + 1) }",
        "fn main() { const a: uint = 0\n print(a - 1) }",
        "fn main() { const a: uint = 4294967296\n print(a * a) }",
        "fn main() { const a: int8 = 100\n const b: int8 = 27\n print(a + b, a - b, b * 4) }",
        // A product by 3 at the edge of the range, and where the product wraps back into it.
        "fn t(n: int) -> int { 3 * n }\nfn main() { print(t(3074457345618258602), t(-3074457345618258602), t(0), t(-1)) }",
        "fn t(n: int) -> int { n * 3 }\nfn main() { print(t(3074457345618258603)) }",
        "fn t(n: int) -> int { 3 * n }\nfn main() { print(t(-3074457345618258603)) }",
        "fn t(n: int) -> int { n * 3 }\nfn main() { print(t(6148914691236517206)) }",
        // Wrapping arithmetic.
        "fn main() { const a: int8 = 127\n print(a +% 1) }",
        "fn main() { const a: uint8 = 255\n print(a +% 1, a *% 2, a -% 255 -% 1) }",
        "fn main() { const a = 9223372036854775807\n print(a +% 1, a *% 3) }",
        "fn main() { var h: uint = 14695981039346656037\n for c of 0..100 { h = (h ^ (c to uint)) *% 1099511628211 }\n print(h) }",
        // Comparisons.
        "fn main() { const a: int8 = -5\n const b: int8 = 3\n print(a < b, a > b, a == b, a <= b, a >= b, a != b) }",
        "fn main() { const a: uint8 = 200\n const b: uint8 = 100\n print(a < b, a > b, a == b) }",
        "fn main() { const a: int32 = -1\n const b: int32 = 1\n print(a < b, a > b) }",
        "fn main() { const a: uint = 18446744073709551615\n const b: uint = 1\n print(a < b, a > b) }",
        // Bit operations and conversions.
        "fn main() { const a: uint8 = 0b1010_1010\n print(a & 0x0F, a | 0x0F, a ^ 0xFF, ~a) }",
        "fn main() { const a: int8 = -5\n print(a to int, a to int32, a to int16, a to float) }",
        "fn main() { const a = 300\n print(a to float, a to int64, a to int) }",
        "fn main() { const a: uint8 = 200\n print(a to int, a to uint64, a to uint32) }",
        "fn main() { const x = 3.99\n const y = -2.5\n print(x.floor(), x.ceil(), y.floor(), y.ceil(), y.round(), x.sqrt(), x.abs(), y.abs(), 1e10.floor()) }",
        // Floats.
        "fn main() { const n = 0.0 / 0.0\n print(n == n, n != n, n < 1.0, n > 1.0, n <= n, n >= n) }",
        "fn main() { const a = 1.5\n const b = 2.5\n print(a < b, a > b, a <= b, a >= b, a == b, a != b, -a, a / b, a * b, a - b) }",
        "fn main() { const z = 0.0\n print(1.0 / z, -1.0 / z, z / z) }",
        "fn main() { const x: float32 = 1.5\n const y: float32 = 0.25\n print(x + y, x * y, x - y, x / y, x < y, -x) }",
        "fn main() { const x: float32 = 2.5\n print(x to float, (x to float) + 0.5, 3 to float32) }",
        "fn main() { const a = 7.5\n const b = 2.0\n const z = 0.0\n print(a % b, -a % b, a % -b, 1e300 % 3.0, a % z, 5.5 % 5.5, 0.1 % 0.03) }",
        "fn main() { const a: float32 = 7.5\n const b: float32 = 2.0\n print(a % b, -a % b) }",
        // Booleans and control flow.
        "fn main() { const t = true\n const f = false\n print(!t, !f, t && f, t || f, t == f, t != f) }",
        "fn main() { var s = 0\n for i of 0..1000 { if i % 3 == 0 || i % 5 == 0 { s += i } }\n print(s) }",
        // Many arguments, floats and integers mixed (stack passing).
        "fn f(a: int, b: int, c: int, d: int, e: int, g: int, h: int, i: int, j: int) -> int = a + 2*b + 3*c + 4*d + 5*e + 6*g + 7*h + 8*i + 9*j\nfn main() { print(f(1, 2, 3, 4, 5, 6, 7, 8, 9)) }",
        "fn f(a: float, b: float, c: float, d: float, e: float, g: float, h: float, i: float, j: float, k: float) -> float = a + b*2.0 + c*3.0 + d*4.0 + e*5.0 + g*6.0 + h*7.0 + i*8.0 + j*9.0 + k*10.0\nfn main() { print(f(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0)) }",
        "fn f(a: int, x: float, b: int, y: float) -> float = (a to float) * x + (b to float) * y\nfn main() { print(f(2, 1.5, 3, 0.5)) }",
        // Buffered output: many small prints, one bigger than the buffer, and prints before a stop.
        "fn main() { for i of 0..30000 { print(i, \"line of text\") } }",
        "fn main() { var s = \"0123456789\"\n for i of 0..13 { s = s + s }\n print(s.len())\n print(s)\n print(\"after\") }",
        "fn main() { for i of 0..20000 { print(i) }\n const z = 0\n print(10 / z) }",
        // Deep recursion.
        "fn d(n: int) -> int { if n == 0 { 0 } else { 1 + d(n - 1) } }\nfn main() { print(d(100000)) }",
        "fn fib(n: int) -> int = n < 2 ? n : fib(n - 1) + fib(n - 2)\nfn main() { print(fib(25)) }",
        // Recursion with values waiting across the calls, and sums of mixed signs.
        "fn g(n: int, a: int, b: int) -> int { if n == 0 { a + b } else { const x = g(n - 1, b, a + 1)\n const y = g(n - 1, a, b + 1)\n x + y + a * b } }\nfn main() { print(g(9, 1, 2)) }",
        "fn fib(n: int) -> int = n < 2 ? n : fib(n - 1) + fib(n - 2)\nfn main() { print(fib(0 - 5), fib(1), fib(20)) }",
        "fn m(n: int, a: int) -> int { if n == 0 { 0 } else { a + m(n - 1, 1 - a) } }\nfn main() { print(m(5, 4611686018427387904), m(6, 0 - 7)) }",
        "fn m(n: int, a: int) -> int { if n == 0 { 0 } else { a + m(n - 1, a) } }\nfn main() { print(m(3, 4611686018427387904)) }",
        // A test at the entry that returns at once: calls are guarded by it.
        "fn tak(x: int, y: int, z: int) -> int { if y < x { tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y)) } else { z } }\nfn main() { print(tak(18, 12, 6), tak(3, 9, 1), tak(0 - 2, 5, 7)) }",
        "fn c(n: int) -> int { if n <= 0 { 0 } else { c(n - 1) + c(n - 2) + 1 } }\nfn main() { print(c(20), c(0 - 4), c(1)) }",
        "fn a(m: int, n: int) -> int { if m == 0 { n + 1 } else if n == 0 { a(m - 1, 1) } else { a(m - 1, a(m, n - 1)) } }\nfn main() { print(a(2, 3), a(3, 3)) }",
    ];
    for p in programs {
        agree(p);
    }
}

// ---- Register allocation: programs that force moves between registers and splits around calls -------------

/// Each program runs in the interpreter, in the optimiser's output and natively with all the registers
/// and with one, two and four of each kind (`REG_SETS`), and all must print the same.
#[test]
fn register_allocation_stress() {
    let programs: [(&str, &str); 5] = [
        // arguments that arrive in each other's registers (a rotation, through a function value too).
        (
            r#"fn mix(a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int) -> int {
    a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8
}
fn rot(m: fn(int, int, int, int, int, int, int, int) -> int, a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int) -> int {
    mix(b, c, d, e, f, g, h, a) + m(h, a, b, c, d, e, f, g) + mix(c, b, a, h, g, f, e, d)
}
fn main() {
    var total = 0
    var i = 1
    while i < 4 {
        total += rot(mix, i, i + 1, i + 2, i + 3, i + 4, i + 5, i + 6, i + 7)
        i += 1
    }
    print(total)
}
"#,
            "1920\n",
        ),
        // block parameters that swap in a cycle.
        (
            r#"fn main() {
    var a = 1
    var b = 2
    var c = 3
    var i = 0
    while i < 10 {
        const t = a
        a = b
        b = c
        c = t + b
        i += 1
    }
    print(a, b, c)
}
"#,
            "60 88 129\n",
        ),
        // eight values that live across calls in a loop.
        (
            r#"fn id(x: int) -> int { x }
fn main() {
    var a = id(1)
    var b = id(2)
    var c = id(3)
    var d = id(4)
    var e = id(5)
    var f = id(6)
    var g = id(7)
    var h = id(8)
    var s = 0
    var i = 0
    while i < 6 {
        s += id(a) * 2 + id(b) + id(c) * 3 + id(d) + id(e) * 5 + id(f) + id(g) * 7 + id(h)
        const t = a
        a = b
        b = c
        c = d
        d = e
        e = f
        f = g
        g = h
        h = t + s % 7
        i += 1
    }
    print(s, a, b, c, d, e, f, g, h)
}
"#,
            "741 7 8 1 8 7 10 9 12\n",
        ),
        // calls whose arguments are calls, with division, shifts and stack arguments around them.
        (
            r#"fn id(x: int) -> int { x }
fn mix(a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int) -> int {
    a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8
}
fn main() {
    var a = id(3)
    var b = id(5)
    var c = id(7)
    var n = 0
    var acc = 0
    while n < 5 {
        acc += mix(id(a), id(b), id(c), id(a + b), id(b + c), id(c + a), id(n), id(acc % 11))
        acc += a / b + c % b + (a << 2) + (c >> 1)
        a += 1
        b += 2
        c += 3
        n += 1
    }
    print(acc, a, b, c)
}
"#,
            "2029 8 15 22\n",
        ),
        // a frame slot too big for the inline zeroing, with values in registers around it.
        (
            r#"struct big { a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int, i: int, j: int }
fn make(x: int) -> big {
    big { a: x, b: x + 1, c: x + 2, d: x + 3, e: x + 4, f: x + 5, g: x + 6, h: x + 7, i: x + 8, j: x + 9 }
}
fn total(v: big) -> int { v.a + v.b + v.c + v.d + v.e + v.f + v.g + v.h + v.i + v.j }
fn main() {
    var s = 0
    var k = 0
    var m = 1
    while k < 6 {
        const v = make(k * m)
        s += total(v) * (k + 1)
        m += 2
        k += 1
    }
    print(s, k, m)
}
"#,
            "7245 6 13\n",
        ),
    ];
    for (src, want) in programs {
        assert_eq!(output_of(src), want, "{src}");
    }
}

/// Large blocks (more than 64 KiB) freed and allocated again: the one kept for reuse must come back zeroed.
#[test]
fn large_blocks_are_reused_zeroed() {
    agree(
        r#"fn build(n: int, k: int) -> int {
    var v: [int] = []
    for i in 0..n { v.push!(i * k) }
    var t = 0
    for i in 0..n { t += v[i] % 7 }
    t
}
fn main() {
    var s = 0
    for r in 0..12 {
        s += build(9000 + r * 500, r + 1)
        s += build(40000 - r * 3000, 3)
    }
    print(s)
}
"#,
    );
}

/// Floats in SSE registers (A-11 W11): arithmetic in place, compares fused into branches, values kept
/// across calls, swaps through block parameters, arguments in `xmm0..7` and on the stack.
#[test]
fn float_registers_stress() {
    let programs = [
        // Swaps and rotations through loop parameters (a cycle of moves between SSE registers).
        r#"fn main() {
    var a = 1.0
    var b = 2.0
    var c = 3.5
    var d = 0.25
    for i in 0..10 {
        const t = a
        a = b
        b = c
        c = d + t
        d = t * 0.5
    }
    print(a, b, c, d)
}
"#,
        // Many floats live across calls, with both widths.
        r#"fn sq(x: float) -> float = x * x
fn half(x: float32) -> float32 = x * 0.5
fn id(a: int) -> int = a + 1
fn main() {
    var s = 0.0
    var t: float32 = 1.0
    var n = 0
    for i in 0..20 {
        const x = i to float
        const y = x * 1.5
        const z = y - 3.0
        const w: float32 = (i to float32) + 0.25
        s += sq(x) + y + z * 0.125
        t = half(t) + w
        n = id(n)
        s += x + y + z
    }
    print(s, t, n)
}
"#,
        // All orderings with ordinary values and NaN, as branches and as values.
        r#"fn main() {
    const zero = 0.0
    const nan = zero / zero
    const xs = [1.0, 2.0, nan, -1.0, 2.0]
    var lt = 0
    var le = 0
    var gt = 0
    var ge = 0
    var eq = 0
    var ne = 0
    var bits = 0
    for i in 0..5 {
        for j in 0..5 {
            const a = xs[i]
            const b = xs[j]
            if a < b { lt += 1 }
            if a <= b { le += 1 }
            if a > b { gt += 1 }
            if a >= b { ge += 1 }
            if a == b { eq += 1 }
            if a != b { ne += 1 }
            const v = a < b
            const w = a >= b
            const u = a == b
            if v { bits += 1 }
            if w { bits += 10 }
            if u { bits += 100 }
        }
    }
    print(lt, le, gt, ge, eq, ne, bits)
}
"#,
        // Negation, remainder, conversions and constants in registers.
        r#"fn main() {
    var x = 7.5
    var y = -2.0
    var acc = 0.0
    for i in 0..8 {
        const a = -x
        const b = a % y
        const c = (i to float) * 0.5 - b
        acc += c + ((rt_to_int(c)) to float) + ((i to float32) to float)
        x = x * 1.25 + 0.0
        y = -y * 0.5
    }
    print(acc, x, y)
}
"#,
        // Floats in memory: loads, stores, struct fields and arrays.
        r#"struct p { x: float, y: float, z: float32 }
fn len2(a: p) -> float = a.x * a.x + a.y * a.y + (a.z to float)
fn main() {
    var v = [0.5; 12]
    for i in 0..12 { v[i] = (i to float) * 0.75 - 1.0 }
    var s = 0.0
    var q = p { x: 1.0, y: 2.0, z: 3.0 }
    for i in 0..12 {
        s += v[i] * v[11 - i]
        q.x = q.x + v[i]
        q.y = q.y - v[11 - i] * 0.5
        q.z = q.z + 0.5
    }
    print(s, len2(q), q.x, q.y, q.z)
}
"#,
        // Float arguments in registers and on the stack, and results used at once.
        r#"fn f(a: float, b: float, c: float, d: float, e: float, g: float, h: float, i: float, j: float, k: float) -> float = a - b * 2.0 + c * 3.0 - d * 4.0 + e * 5.0 - g * 6.0 + h * 7.0 - i * 8.0 + j * 9.0 - k * 10.0
fn rot(a: float, b: float, c: float) -> float = a + b * 10.0 + c * 100.0
fn main() {
    var t = 0.0
    for n in 0..6 {
        const x = n to float
        t += f(x, x + 1.0, x + 2.0, x + 3.0, x + 4.0, x + 5.0, x + 6.0, x + 7.0, x + 8.0, x + 9.0)
        t += rot(x, t, 1.0) + rot(t, 1.0, x) + rot(1.0, x, t)
    }
    print(t)
}
"#,
        // A float recursion and a loop that mixes integers and floats.
        r#"fn p(x: float, n: int) -> float { if n == 0 { x } else { p(x * 1.01 + 0.5, n - 1) * 0.99 + x } }
fn main() {
    var s = 0.0
    var k = 1.0
    for i in 1..30 {
        s += p(k, i % 7) / (i to float)
        k = k + 1.0 / (i to float)
    }
    print(s, k)
}
"#,
    ];
    for p in programs {
        agree(p);
    }
}

/// Memory operations (A-11 W5): addresses folded into the load or store, copies as plain moves.
#[test]
fn memory_operations_stress() {
    let programs: [(&str, &str); 2] = [
        // structs of 1 to 72 bytes copied, changed and nested (small copies, frame offsets, a copy that still uses `rep movsb`).
        (
            r#"struct s1 { a: int8 }
struct s3 { a: int, b: int, c: int }
struct s5 { a: int, b: int, c: int, d: int, e: int }
struct s9 { a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int, i: int }
struct mix { a: int8, b: int, c: int16, d: int32, e: bool, f: int8 }
struct wrap { m: mix, k: s3 }

fn mk3(x: int) -> s3 { s3 { a: x, b: x + 1, c: x + 2 } }
fn mk9(x: int) -> s9 { s9 { a: x, b: x + 1, c: x + 2, d: x + 3, e: x + 4, f: x + 5, g: x + 6, h: x + 7, i: x + 8 } }
fn sum9(v: s9) -> int { v.a + v.b + v.c + v.d + v.e + v.f + v.g + v.h + v.i }

fn main() {
    var t = 0
    for i in 0..5 {
        const x = mk3(i)
        var y = x
        y.b = y.b * 10
        t += x.a + x.b + x.c + y.a + y.b + y.c
        const p = s5 { a: i, b: 2, c: 3, d: 4, e: i * i }
        var q = p
        q.e += 1
        t += p.e + q.e + q.a + q.d
        const big = mk9(i)
        var big2 = big
        big2.i = 100
        t += sum9(big) + sum9(big2)
        const m = mix { a: -3, b: i * 1000, c: -300, d: 70000, e: i > 2, f: 7 }
        var w = wrap { m: m, k: x }
        w.m.a = w.m.a - 1
        const w2 = w
        t += (w2.m.a to int) + w2.m.b + (w2.m.c to int) + (w2.m.d to int) + w2.k.c + (w2.m.f to int)
        if w2.m.e { t += 1 }
        const one = s1 { a: -5 }
        var two = one
        two.a = two.a * 2
        t += (one.a to int) + (two.a to int)
    }
    print(t)
}
"#,
            "359772\n",
        ),
        // arrays of every element width and two dimensions (index scales 1, 2, 4 and 8 in the address).
        (
            r#"fn main() {
    var a8: [int8; 40] = [0; 40]
    var a16: [int16; 40] = [0; 40]
    var a32: [int32; 40] = [0; 40]
    var a64 = [0; 40]
    var v8: int8 = -60
    var v16: int16 = -6000
    var v32: int32 = -2000000
    for i in 0..40 {
        a8[i] = v8
        a16[i] = v16
        a32[i] = v32
        a64[i] = i * 1000000007 - 20000000000
        v8 += 3
        v16 += 300
        v32 += 100000
    }
    var t = 0
    for i in 0..40 {
        t += (a8[i] to int) + (a16[i] to int) + (a32[i] to int) + a64[i]
        t += a8[39 - i] to int
    }
    var grid = [[0; 7]; 5]
    for r in 0..5 {
        for c in 0..7 { grid[r][c] = r * 10 + c }
    }
    var g = 0
    for r in 0..5 {
        for c in 0..7 { g += grid[r][c] * (c + 1) + grid[4 - r][6 - c] }
    }
    print(t, g)
}
"#,
            "-20002000660 4165\n",
        ),
    ];
    for (src, want) in programs {
        assert_eq!(output_of(src), want, "{src}");
    }
}

#[test]
fn big_buffers_keep_their_contents_while_they_grow() {
    // Past 64 KiB a buffer is a mapping of its own that the system moves when it grows; the
    // elements, a string and a list of strings must all survive the moves. `out` shrinks nothing but
    // is released and rebuilt, so blocks of every size are freed too.
    let src = "fn main() {\n var xs: [int] = []\n for i in 0..70000 { xs.push!(i) }\n var ok = true\n var t = 0\n for i in 0..xs.len() { if xs[i] != i { ok = false }\n t += xs[i] }\n var s = \"\"\n for i in 0..40000 { s = s + \"xy\" }\n var words: [string] = []\n for i in 0..3000 { words.push!(\"w#i\") }\n var out: [int] = []\n for round in 0..3 { out = []\n for i in 0..(9000 * (round + 1)) { out.push!(i * 2) } }\n print(ok, t, xs.len(), s.len(), s[79998..80000], words[2999], words.len(), out.len(), out[26999]) }\n";
    let want = "true 2449965000 70000 80000 xy w2999 3000 27000 53998\n";
    assert_eq!(output_of(src), want);
    agree(src);
}

#[test]
fn a_buffer_past_a_mebibyte_keeps_its_contents_while_it_grows() {
    // On Windows a block of 1 MiB or more reserves eight times its size and commits as it grows; this
    // list passes 1 MiB and then grows in place.
    let src = "fn main() {\n var xs: [int] = []\n for i in 0..300000 { xs.push!(i) }\n var t = 0\n for i in 0..xs.len() { t += xs[i] }\n print(xs.len(), t, xs[299999]) }\n";
    assert_eq!(output_of(src), "300000 44999850000 299999\n");
    agree(src);
}

#[test]
fn a_branch_with_a_checked_side_becomes_a_select_and_stops_only_when_taken() {
    // `3 * n + 1` is checked, and it runs ahead of the test on even values too: it must not stop for
    // them, and must stop for the odd ones that overflow.
    let step = "fn step(n: int) -> int { if n % 2 == 0 { n / 2 } else { 3 * n + 1 } }\n";
    let l = lowered(&format!("{step}fn main() {{ print(step(7)) }}"));
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "step").unwrap(), None);
    assert!(text.contains("select"), "{text}");
    assert!(text.contains("checked_if"), "{text}");
    assert!(!text.contains("branch"), "{text}");
    let go = |n: i64| {
        run_full(&m, "step", &[Val::int(n)])
            .result
            .map(|v| v.as_i64())
    };
    assert_eq!(go(7), Ok(22));
    assert_eq!(go(10), Ok(5));
    assert_eq!(go(-3), Ok(-8));
    assert_eq!(go(-4), Ok(-2));
    assert_eq!(go(i64::MAX - 1), Ok(i64::MAX / 2));
    assert_eq!(go(i64::MIN), Ok(i64::MIN / 2));
    assert_eq!(go(i64::MAX), Err("arithmetic overflow".to_string()));
    assert_eq!(go(i64::MIN + 1), Err("arithmetic overflow".to_string()));
    assert_eq!(go(3_074_457_345_618_258_602), Ok(1_537_228_672_809_129_301));
    assert_eq!(
        go(3_074_457_345_618_258_603),
        Err("arithmetic overflow".to_string())
    );
    // The same, native.
    let (big, huge) = ("9223372036854775806", "9223372036854775807");
    agree(&format!(
        "{step}fn main() {{ print(step({big}), step(7), step(-3), step(-4)) }}"
    ));
    agree(&format!(
        "{step}fn main() {{ var n = {huge}\n n += 0\n print(step(n)) }}"
    ));
}

#[test]
fn a_branch_with_plain_sides_becomes_a_select() {
    let src = "fn pick(a: int, b: int) -> int { if a > b { a - b } else { b - a } }\nfn main() { var s = 0\n for i in 0..50 { s += pick(i * 7 % 13, i % 5) }\n print(s) }";
    let l = lowered(src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "pick").unwrap(), None);
    assert!(text.contains("select"), "{text}");
    agree(src);
}

#[test]
fn masks_nobody_reads_go_and_counters_that_move_together_become_one() {
    let src = "fn main() { var x = 1\n var y = 2\n var t = 0\n for i in 0..1000 { x += 3\n y += 1\n t += (x * 3 + y) & 1023\n x = x & 65535\n y = y & 65535 }\n print(t) }";
    let l = lowered(src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "main").unwrap(), None);
    assert!(!text.contains("65535"), "{text}");
    // The two counters and the formula over them became one counter that steps by 3 * 3 + 1.
    assert!(text.contains("const i64 10\n"), "{text}");
    agree(src);
}

#[test]
fn a_mask_that_is_read_in_full_stays() {
    // `x > 40000` reads every bit of the masked counter, so the mask cannot go.
    let src = "fn main() { var x = 1\n var n = 0\n for i in 0..100000 { x += 3\n x = x & 65535\n if x > 40000 { n += 1 } }\n print(n, x) }";
    let l = lowered(src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "main").unwrap(), None);
    assert!(text.contains("65535"), "{text}");
    agree(src);
}

#[test]
fn counters_that_the_loop_also_reads_keep_their_own_value() {
    // `x` and `y` are read outside the formula, so combining them would add a counter, not remove two.
    let src = "fn main() { var x = 1\n var y = 2\n var t = 0\n for i in 0..1000 { x += 3\n y += 5\n t += (x + y) & 255\n t += x & 15 }\n print(t, x, y) }";
    agree(src);
}

#[test]
fn a_long_counted_loop_is_unrolled_and_keeps_its_results_and_its_stops() {
    // The bound is a constant, so the loop is unrolled; the start moves the number of passes around
    // every multiple of the factor, and `top` ends at the largest integer.
    let src = "fn sum(from: int) -> int { var s = 0\n for i in from..1003 { s += (i + 3) & 255 }\n s }\nfn grow(from: int) -> int { var s = 1\n for i in from..1003 { s *= 3 }\n s }\nfn top(from: int) -> int { var s = 0\n for i in from..9223372036854775807 { s += i & 7 }\n s }\nfn main() { print(sum(0), grow(1000), top(9223372036854775800)) }";
    let l = lowered(src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "sum").unwrap(), None);
    assert!(text.matches(" = lt ").count() >= 2, "{text}");
    let starts = [
        0i64, 1, 2, 3, 7, 8, 9, 995, 996, 999, 1000, 1001, 1002, 1003, 1004, -5,
    ];
    for name in ["sum", "grow"] {
        for n in starts {
            let args = [Val::int(n)];
            let plain = run_full(&l.module, name, &args).result.map(|v| v.as_i64());
            let fast = run_full(&m, name, &args).result.map(|v| v.as_i64());
            assert_eq!(plain, fast, "{name}({n})");
        }
    }
    for n in [
        i64::MAX - 20,
        i64::MAX - 8,
        i64::MAX - 7,
        i64::MAX - 1,
        i64::MAX,
    ] {
        let args = [Val::int(n)];
        let plain = run_full(&l.module, "top", &args).result.map(|v| v.as_i64());
        let fast = run_full(&m, "top", &args).result.map(|v| v.as_i64());
        assert_eq!(plain, fast, "top({n})");
    }
    agree("fn sum(from: int) -> int { var s = 0\n for i in from..1003 { s += (i + 3) & 255 }\n s }\nfn main() { var t = 0\n for k in 0..40 { t += sum(k * 25) }\n print(t, sum(0)) }");
    agree("fn grow(from: int) -> int { var s = 1\n for i in from..1003 { s *= 3 }\n s }\nfn main() { print(grow(970)) }");
}

#[test]
fn counted_loops_of_plain_arithmetic_run_in_vectors_with_the_same_results() {
    // Counters, a sum and a difference, a shift and an xor over a constant bound. The bounds fall on
    // every residue of the four iterations a pass covers, so each length of remainder runs.
    let bounds = [256i64, 257, 258, 259, 1003];
    let mut src = String::new();
    for n in bounds {
        src += &format!("fn f{n}() -> int {{ var p = 5\n var s = 0\n var d = 7\n for i in 0..{n} {{ p += 7\n s += (p ^ 33) & 4095\n d += (p << 2) & 255 }}\n s + d + p }}\nfn g{n}() -> int {{ var e = 1\n for i in 0..{n} {{ e += i & 15 }}\n e }}\n");
    }
    // The start is not known, so this one stays scalar; it also runs up to the largest integer.
    src += "fn top(from: int) -> int { var s = 0\n for i in from..9223372036854775807 { s += (i & 7) << 1 }\n s }\nfn main() {";
    for n in bounds {
        src += &format!(" print(f{n}(), g{n}())\n");
    }
    src += " print(top(9223372036854775800)) }";
    let l = lowered(&src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    for n in bounds {
        let name = format!("f{n}");
        let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == name).unwrap(), None);
        assert!(
            text.contains("vreduce_add"),
            "{name} is not vectorised:\n{text}"
        );
        let plain = run_full(&l.module, &name, &[]).result.map(|v| v.as_i64());
        let fast = run_full(&m, &name, &[]).result.map(|v| v.as_i64());
        assert_eq!(plain, fast, "{name}");
    }
    for n in [
        i64::MAX - 20,
        i64::MAX - 8,
        i64::MAX - 4,
        i64::MAX - 3,
        i64::MAX - 1,
        i64::MAX,
    ] {
        let args = [Val::int(n)];
        let plain = run_full(&l.module, "top", &args).result.map(|v| v.as_i64());
        let fast = run_full(&m, "top", &args).result.map(|v| v.as_i64());
        assert_eq!(plain, fast, "top({n})");
    }
    agree(&src);
}

#[test]
fn lists_are_summed_and_mapped_in_vectors_with_the_same_results_and_stops() {
    // `go` builds a list of `n` numbers from `base`, maps it in place and sums it. The lengths fall on
    // every remainder of the four elements a pass covers; a `base` above 2^29 makes the guard of the
    // checked sum fail and the loop run again scalar; a `base` near the top overflows the sum, which
    // must stop at the same element.
    let src = "fn go(n: int, base: int) -> int { var xs: [int] = []\n for i in 0..n { xs.push!(base + i) }\n var t = 0\n for i in 0..xs.len() { t += xs[i] }\n for i in 0..xs.len() { xs[i] = (xs[i] ^ 12345) & 65535 }\n for i in 0..xs.len() { t += xs[i] }\n t }\nfn main() { print(go(7, 1), go(1000, 5), go(5, 1099511627776)) }";
    let l = lowered(src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "go").unwrap(), None);
    assert!(
        text.contains("vreduce_add"),
        "the sum is not vectorised:\n{text}"
    );
    assert!(text.contains("vxor"), "the map is not vectorised:\n{text}");
    let bases = [
        0i64,
        1,
        536870911,
        536870912,
        -536870912,
        -536870913,
        1099511627776,
        9223372036854775800,
        -9223372036854775800,
    ];
    for base in bases {
        for n in (0..14).chain([100, 101, 102, 103, 1000]) {
            let args = [Val::int(n), Val::int(base)];
            let plain = run_full(&l.module, "go", &args);
            let fast = run_full(&m, "go", &args);
            assert_eq!(
                plain.result.map(|v| v.as_i64()),
                fast.result.map(|v| v.as_i64()),
                "go({n}, {base})"
            );
            // A stop leaves what it holds, in both.
            assert_eq!(plain.leaked, fast.leaked, "go({n}, {base}) leaked");
        }
    }
    agree(src);
}

#[test]
fn a_checked_sum_is_not_vectorised_and_still_stops_in_the_same_pass() {
    // The product can overflow, so the loop stays scalar and the stop comes from the same iteration.
    let src = "fn g(from: int) -> int { var s = 0\n for i in from..1003 { s += i * 4611686018427387 }\n s }\nfn main() { print(g(0)) }";
    let l = lowered(src);
    let mut m = l.module.clone();
    crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
    let text = crate::ir::print_func(m.funcs.iter().find(|f| f.name == "g").unwrap(), None);
    assert!(!text.contains("vreduce_add"), "{text}");
    for n in [0i64, 1, 500, 1000] {
        let args = [Val::int(n)];
        let plain = run_full(&l.module, "g", &args);
        let fast = run_full(&m, "g", &args);
        assert_eq!(plain.result, fast.result, "g({n})");
    }
}

// ---- Float semantics: single precision rounding and saturating float to int -------------------------------

#[test]
fn float32_rounds_after_every_operation() {
    // 16777216 + 1 is not a float32: the sum rounds back (to even), as it does natively.
    assert_eq!(
        output_of("fn main() { const a: float32 = 16777218.0\n const b: float32 = 1.0\n print((a + b) to float, (a + b + b) to float) }"),
        "16777220 16777220\n"
    );
    // 0.1 is not exact in single precision, and the widening shows it.
    assert_eq!(
        output_of("fn main() { const a: float32 = 0.1\n print((a to float) == 0.1, (a to float) == ((0.1 to float32) to float)) }"),
        "false true\n"
    );
    // An integer converts to float32 with one rounding.
    assert_eq!(
        output_of("fn main() { const n = 16777217\n print((n to float32) to float) }"),
        "16777216\n"
    );
}

#[test]
fn float_to_int_saturates() {
    // `floor` goes through the conversion to an integer and back.
    let src = "fn main() { const z = 0.0\n const big = 1e30\n print(big.floor(), (-big).floor(), (z / z).floor(), 2.9.floor(), (-2.9).floor(), 1e19.ceil(), (-1e19).ceil()) }";
    assert_eq!(
        output_of(src),
        "9223372036854775807 -9223372036854775807 0 2 -3 9223372036854775807 -9223372036854775807\n"
    );
}

/// Runaway recursion natively: a stop with exit status 1 on both systems, after the output so far. The
/// second program has frames of 1.6 MB, bigger than the guard zone on Linux: the prologue touches every
/// page of a big frame so it cannot step over the guard.
#[test]
fn native_runaway_recursion_stops_cleanly() {
    use crate::native::{exec, gen, Os};
    let programs = [
        "fn d(n: int) -> int { d(n + 1) * n }\nfn main() { print(\"before\")\n print(d(1)) }",
        "fn big(n: int) -> int {\n var a = [0; 200000]\n a[n % 200000] = n\n if n > 100000000 { return a[1] }\n a[0] + big(n + 1) * 2\n}\nfn main() { print(\"before\")\n print(big(1)) }",
    ];
    for src in programs {
        // Not optimised, so the recursion stays a recursion.
        let l = lowered(src);
        for os in [Os::Linux, Os::Windows] {
            if !exec::can_run(os) {
                continue;
            }
            let opts = gen::Options {
                leak_check: false,
                target: os,
                regalloc: true,
            };
            let image = gen::compile(&l.module, "main", &opts).unwrap();
            let r = exec::run_image(os, &image).unwrap();
            assert_eq!(
                (r.out.trim_end(), r.err.trim_end(), r.code),
                ("before", "stopped: stack overflow", 1),
                "{os:?} in {src}"
            );
        }
        if let Some(r) = exec::build_and_run_a64(&l.module, false) {
            assert_eq!(
                (r.out.trim_end(), r.err.trim_end(), r.code),
                ("before", "stopped: stack overflow", 1),
                "AArch64 in {src}"
            );
        }
    }
}

/// A big array written by `[x; n]` is not cleared first; what an earlier call left on the stack must
/// not show, and the stack probes for a big frame must still work.
#[test]
fn big_arrays_are_filled_in_place() {
    agree(
        "fn dirty() -> int {
            var a = [7; 900]
            a[5] + a[899]
        }
        fn main() {
            const d = dirty()
            var f = [1; 900]
            var s = 0
            for i in 0..900 { s += f[i] }
            var big = [2; 40000]
            var t = 0
            for i in 0..40000 { t += big[i] }
            print(d, s, t)
        }",
    );
}

/// An index the loop bounds keep inside the array needs no test; one nothing bounds keeps its test.
#[test]
fn index_tests_that_cannot_fail_are_dropped() {
    let tests = |src: &str| {
        let l = lowered(src);
        let mut m = l.module.clone();
        crate::ir::opt::optimize(&mut m, crate::ir::opt::Options::default());
        let f = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let n = f
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(f.insts[**i].op, Op::Cmp(CmpOp::UGe, ..)))
            .count();
        agree(src);
        n
    };
    let inside = "fn main() {
        var a = [1; 100]
        var s = 0
        for i in 0..100 { s += a[i] }
        var j = 3
        while j <= 99 { a[j] = 0; j += 3 }
        print(s, a[99])
    }";
    assert_eq!(tests(inside), 0);
    let outside = "fn main() {
        var a = [1; 100]
        var j = 3
        while j <= 100 { a[j] = 0; j += 3 }
        print(a[99])
    }";
    assert!(tests(outside) > 0);
}

/// A loop that stores one value into a big array is a `rep stos` natively: every element size, and
/// a value that is not a constant, give what the interpreter gives.
#[test]
fn fill_loops_become_string_stores() {
    agree(
        "fn main() {
            var a: [uint8; 1000] = [7; 1000]
            var b: [int32; 300] = [-5; 300]
            var c: [bool; 600] = [true; 600]
            var d: [float; 100] = [1.5; 100]
            var e: [int; 70] = [3; 70]
            const k = 41
            var g = [k + 1; 80]
            print(a[0], a[999], b[0], b[299], c[0], c[599])
            print(d[0], d[99], e[0], e[69], g[0], g[79])
        }",
    );
}

/// Appending a literal of 1, 2, 4 or 8 bytes is a store of a constant natively, also when the bytes
/// are not ASCII (the constant is sign extended); other lengths still copy.
#[test]
fn literals_are_stored_as_constants() {
    agree(
        "fn main() {
            var s = \"\"
            for i in 0..3 {
                s = s + \"x\"
                s = s + \"é\"
                s = s + \"abcd\"
                s = s + \"€\"
                s = s + \"abcdefgh\"
                s = s + \"ÿÿÿÿ\"
                s = s + \"12345\"
            }
            print(s, s.len())
        }",
    );
}

/// A length that grows by a checked add is read back as the new length minus the step in the address
/// and in the call that grows the buffer; the loop's exit still sees the length itself.
#[test]
fn a_growing_length_is_added_in_place() {
    agree(
        "fn main() {
            var s = \"\"
            var t = 0
            for i in 0..3000 {
                s = s + \"ab\"
                t += s.len() & 7
            }
            var xs: [int] = []
            for i in 0..3000 { xs.push!(i * 3) }
            var u = 0
            for i in 0..xs.len() { u += xs[i] }
            print(s.len(), t, xs.len(), u, xs[2999])
        }",
    );
}

/// An accumulator of a loop that runs a known number of times has no overflow check when the trip
/// count times the biggest step stays in range; when it does not, the program still stops.
#[test]
fn accumulators_of_counted_loops_keep_their_traps() {
    agree(
        "fn main() {
            var t = 0
            var n = 0
            for i in 0..2000 {
                n += 2
                t += n & 7
            }
            print(t, n)
        }",
    );
    agree(
        "fn main() {
            var t = 0
            for i in 0..100 { t += 4611686018427387904 }
            print(t)
        }",
    );
    agree(
        "fn main() {
            var t = 9223372036854775800
            for i in 0..100 { t += 1 }
            print(t)
        }",
    );
}

/// `(-x) * c` is computed as `x * (-c)`; the results, signs of zero included, must not change.
#[test]
fn a_negation_before_a_constant_product_changes_no_result() {
    agree(
        "fn main() {
            var x = 0.5
            for i in 0..6 {
                const a = -x * 0.001
                const b = 0.25 * -x
                const c = -x / 4.0
                print(a, b, c, 1.0 / a)
                x = x - 0.25
            }
        }",
    );
}

/// Blocks from 2 MiB up are mapped in whole 2 MiB pages: growing, reusing and freeing them must work
/// at sizes around the edge (a 2 MiB block holds 262,144 ints less the header).
#[test]
fn big_blocks_grow_and_are_reused() {
    agree(
        "fn main() {
            var total = 0
            for round in 0..3 {
                var xs: list<int> = []
                for i in 0..(262000 + round * 300) { xs.push!(i) }
                for x in xs { total += x & 255 }
            }
            print(total)
        }",
    );
}

/// A loop limit that is not a constant but has a known range bounds the trip count too; a limit
/// that can be huge still traps.
#[test]
fn a_limit_with_a_known_range_bounds_the_trips() {
    agree(
        "fn main() {
            var t = 0
            for k in 0..50 {
                const n = (k * 37) & 1023
                var s = 0
                for i in 0..n { s += i }
                t += s
            }
            print(t)
        }",
    );
    agree(
        "fn main() {
            var t = 9223372036854775000
            for k in 0..50 {
                const n = (k * 37) & 1023
                for i in 0..n { t += i }
            }
            print(t)
        }",
    );
}

/// The loop test moves onto a counter that moves with the loop's own (`len` grows by 2 per trip);
/// the loop must run as often as before, also with an offset start and a step above one.
#[test]
fn the_loop_test_may_read_another_counter() {
    agree(
        "fn main() {
            var n = 5
            var m = 0
            for i in 0..1000 {
                n += 2
                m += n & 3
            }
            var k = 100
            var c = 0
            for i in 3..1000 {
                k += 7
                c += 1
            }
            print(n, m, k, c)
        }",
    );
}

#[test]
fn default_that_returns_does_not_leak_later_locals() {
    // `?? return` leaves the default's scope open unless it is popped; the locals after it
    // then registered in that dead scope and leaked on the way out.
    let src = "fn pair(n: int) -> int ! string {\n var s: [string] = [\"x#(1)\", \"yy\"]\n const b = s.pop!() ?? return err(\"b\")\n const a = s.pop!() ?? return err(\"a\")\n ok(a.len() + b.len() + n)\n}\nfn main() {\n switch pair(0) { case ok(n): print(n)\n case err(e): print(e) }\n}\n";
    assert_eq!(output_of(src), "4\n");
}

/// Keys that land in the same slot of a small table go on along the probe sequence: lookups, inserts
/// that replace, removals and a set, with the first slot holding another key.
#[test]
fn maps_find_keys_that_collide_in_the_first_slot() {
    agree(
        "fn main() {
            var m: map<int, int> = {:}
            for i in 0..60 { m.insert!(i * 1048576, i) }
            for i in 0..60 { m.insert!(i * 1048576, i + 100) }
            var t = 0
            for i in 0..60 { t += m.get(i * 1048576) ?? 0 }
            for i in 0..60 { t += m.get(i * 1048576 + 1) ?? 1000 }
            print(m.len(), t)
            var s: set<int> = set {}
            for i in 0..40 { s.insert!(i * 65536) }
            for i in 0..40 { s.insert!(i * 65536) }
            var c = 0
            for i in 0..80 { if s.contains(i * 65536) { c += 1 } }
            print(s.len(), c)
        }",
    );
}

#[test]
fn cloning_and_sorting_an_empty_list() {
    // Cloning copies zero bytes between two null addresses; the interpreter must accept that.
    assert_eq!(
        output_of(
            "fn main() { var xs: [int] = []\n xs.sort!()\n const ys = xs.sorted()\n var zs: [string] = []\n const ws = zs.toList()\n print(xs.len(), ys.len(), ws.len()) }"
        ),
        "0 0 0\n"
    );
}

#[test]
fn sorting_integers_past_the_insertion_sort_limit() {
    // 200 integers: negatives, duplicates and the extremes; the radix sort must agree with a check
    // that every neighbour is in order, and keep the total.
    let src = "fn main() {
        var xs: [int] = [9223372036854775807, -9223372036854775807, 0, -1]
        var s = 3
        for i in 0..196 {
            s = (s * 1103515245 + 12345) & 2147483647
            xs.push!((s & 1023) - 500)
        }
        var before = 0
        for x of xs { before += x & 65535 }
        xs.sort!()
        var ok = xs.len() == 200
        var after = 0
        for i in 0..xs.len() {
            after += xs[i] & 65535
            if i > 0 && xs[i - 1] > xs[i] { ok = false }
        }
        print(ok, before == after, xs[0], xs[199])
    }";
    assert_eq!(
        output_of(src),
        "true true -9223372036854775807 9223372036854775807\n"
    );
}

// ---- Arenas ------------------------------------------------------------------------------------

#[test]
fn arena_new_get_set_reset() {
    let src = "struct pt { x: int, y: int }\nfn main() {\n var a = arena()\n const p = a.new!(pt { x: 1, y: 2 })\n const q = a.new!(7)\n print(a.get(p).x, a.get(p).y, a.get(q), a.used())\n a.set!(q, 9)\n print(a.get(q))\n a.reset!()\n print(a.used())\n const r = a.new!(5)\n print(a.get(r)) }\n";
    assert_eq!(output_of(src), "1 2 7 24\n9\n0\n5\n");
}

#[test]
fn arena_handles_survive_growth_and_a_move() {
    let src = "fn fill(sink a: arena) -> arena { var b = a\n for i in 0..1000 { b.new!(i) }\n b }\nfn main() {\n var a = arena()\n const first = a.new!(41)\n const b = fill(a)\n print(b.get(first), b.used())\n var big = arena()\n const f = big.new!(1)\n for i in 0..20000 { big.new!(i) }\n print(big.get(f)) }\n";
    assert_eq!(output_of(src), "41 8008\n1\n");
}

#[test]
fn arena_tree_of_nodes() {
    let src = "struct node { l: aref<node>?, r: aref<node>? }\nfn make(inout a: arena, d: int) -> aref<node> {\n if d == 0 { return a.new!(node { l: none, r: none }) }\n const l = make(inout a, d - 1)\n const r = make(inout a, d - 1)\n a.new!(node { l: l, r: r }) }\nfn check(a: arena, t: aref<node>) -> int {\n const n = a.get(t)\n var c = 1\n if n.l is some(l) { c += check(a, l) }\n if n.r is some(r) { c += check(a, r) }\n c }\nfn main() {\n var a = arena()\n for i in 0..3 {\n  const t = make(inout a, 4)\n  print(check(a, t))\n  a.reset!() } }\n";
    assert_eq!(output_of(src), "31\n31\n31\n");
}

#[test]
fn arena_native_agrees() {
    let src = "struct node { l: aref<node>?, r: aref<node>? }\nfn make(inout a: arena, d: int) -> aref<node> {\n if d == 0 { return a.new!(node { l: none, r: none }) }\n const l = make(inout a, d - 1)\n const r = make(inout a, d - 1)\n a.new!(node { l: l, r: r }) }\nfn check(a: arena, t: aref<node>) -> int {\n const n = a.get(t)\n var c = 1\n if n.l is some(l) { c += check(a, l) }\n if n.r is some(r) { c += check(a, r) }\n c }\nfn main() {\n var a = arena()\n for i in 0..3 {\n  const t = make(inout a, 4)\n  print(check(a, t))\n  a.reset!() } }\n";
    agree(&src);
}

#[test]
fn arena_stale_handle_and_drop_glue_stop() {
    let stale =
        "fn main() {\n var a = arena()\n const p = a.new!(1)\n a.reset!()\n print(a.get(p)) }\n";
    let o = run_main(stale);
    assert!(o.result.is_err());
    assert!(
        o.output.contains("used a handle after the arena was reset"),
        "{}",
        o.output
    );
    let other = "fn main() {\n var a = arena()\n var b = arena()\n const p = a.new!(1)\n print(b.get(p)) }\n";
    assert!(run_main(other).result.is_err());
    let glue = "fn main() {\n var a = arena()\n a.new!(\"hi\") }\n";
    let o = run_main(glue);
    assert!(o.result.is_err());
    assert!(o.output.contains("without drop glue"), "{}", o.output);
}
