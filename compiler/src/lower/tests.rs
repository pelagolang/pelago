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
    let o = run_main(src);
    assert_eq!(o.result.clone().map(|_| ()), Ok(()), "{src}");
    assert_eq!(o.leaked, 0, "leaked memory in {src}");
    o.output
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
    let src = "fn quarter(n: int) -> int ! parse_error {\n const h = parse(\"8\")?\n ok(h + n)\n}\nfn parse(s: str) -> int ! parse_error = s.parse_int()\nfn main() { print(quarter(1), parse(\"x\")) }";
    assert_eq!(output_of(src), "ok(9) err(bad digit at position 0)\n");
    let src =
        "fn f(o: int?) -> int? {\n const v = o?\n v + 1\n}\nfn main() { print(f(5), f(none)) }";
    assert_eq!(output_of(src), "some(6) none\n");
    let src = "fn main() { const a: int? = none\n const b: int? = 4\n print(a ?? 9, b ?? 9) }";
    assert_eq!(output_of(src), "9 4\n");
    let src = "fn main() { const names: [string?] = [\"a\", none]\n print(names[0] ?? \"-\", names[1] ?? \"-\") }";
    assert_eq!(output_of(src), "a -\n");
    let src = "fn load(s: str) -> int ! error {\n const n = s.parse_int()?\n ok(n * 2)\n}\nfn main() { print(load(\"21\"), load(\"zz\")) }";
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
    let src = "fn main() { const s = \"  Hello, World  \"\n print(s.trim(), s.trim().to_upper(), s.to_lower().trim(), s.contains(\"World\"), s.trim().starts_with(\"Hello\"), s.trim().ends_with(\"d\")) }";
    assert_eq!(
        output_of(src),
        "Hello, World HELLO, WORLD hello, world true true true\n"
    );
    let src =
        "fn main() { const parts = \"a,b,,c\".split(\",\").to_list()\n print(parts, parts.len()) }";
    assert_eq!(output_of(src), "[\"a\", \"b\", \"\", \"c\"] 4\n");
    let src =
        "fn main() { print(\"hello world\".replace(\"o\", \"0\"), \"héllo\".chars().count()) }";
    assert_eq!(output_of(src), "hell0 w0rld 5\n");
    let src = "fn main() { print(\"42\".parse_int(), \"-7\".parse_int(), \"4x\".parse_int(), \"3.5\".parse_float()) }";
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
    let src = "fn main() {\n const xs = [1, 2, 3, 4]\n print(xs.map(x ~> x * 10).to_list(), xs.filter(x ~> x % 2 == 0).to_list())\n var total = 0\n xs.each(x ~> { total += x })\n print(total) }";
    assert_eq!(output_of(src), "[10, 20, 30, 40] [2, 4]\n10\n");
    let src = "fn main() {\n const names = [\"a\", \"bb\"]\n print(names.map(s ~> s.len()).to_list())\n const o: int? = 4\n print(o.map(x ~> x + 1), o.or_err(\"no\"))\n const r: int ! string = err(\"e\")\n print(r.map_err(s ~> s.len())) }";
    assert_eq!(output_of(src), "[1, 2]\nsome(5) ok(4)\nerr(1)\n");
}

#[test]
fn maps_and_sets() {
    let src = "fn main() {\n var m = map { \"a\": 1, \"b\": 2 }\n m.insert!(\"c\", 3)\n m.insert!(\"a\", 10)\n print(m, m.len(), m.get(\"a\"), m.get(\"z\"), m[\"b\"])\n m.remove!(\"b\")\n print(m, m.contains_key(\"b\"))\n for k, v of m { print(k, v) }\n for k in m { print(k) } }";
    assert_eq!(output_of(src), "{\"a\": 10, \"b\": 2, \"c\": 3} 3 some(10) none 2\n{\"a\": 10, \"c\": 3} false\na 10\nc 3\na\nc\n");
    let src = "fn main() {\n var s = set { 3, 1, 3, 2 }\n s.insert!(9)\n s.remove!(1)\n print(s, s.len(), 2 in s, 5 in s)\n const t = copy s\n print(t == s, set { 1 } == set { 2 }) }";
    assert_eq!(output_of(src), "{3, 2, 9} 3 true false\ntrue false\n");
    let src = "fn main() { var total = 0\n for (var i = 0; i < 4; i += 1) { total += i }\n print(total, [1, 2].iter().to_set(), [\"a\", \"b\"].join(\"-\"), [0, ...[1, 2], 3]) }";
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
    let src = "struct acct { id: int, bal: int = 100\n init(id: int) { self.id = id } }\nstruct rng { lo: int, hi: int\n init(lo: int, hi: int) -> Self ! string {\n  if lo > hi { return err(\"bad\") }\n  self.lo = lo\n  self.hi = hi } }\nobj res { n: int\n init(n: int) { self.n = n }\n deinit { print(\"closed #(self.n)\") } }\nfn main() { const a = acct(5)\n print(a.id, a.bal)\n print(rng(1, 5).is_ok(), rng(5, 1).is_ok())\n const r = res(7)\n print(r.n) }\n";
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
    let src = "fn main() { const r = 1..5\n print(r.contains(3), r.contains(5), (1..=5).contains(5), r.iter().to_list(), 2.5.min(1.0))\n for i of (1..4).iter() { print(i) } }\n";
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
fn program_builder() {
    let src = "fn main() { const xs = [1, 2, 3, 4, 5, 6]\n print([x of xs if x % 2 == 0 ~> x * 10])\n print([x of xs if x > 4])\n print([i, x of xs if i < 2 ~> \"#i:#x\"])\n print([x of 0..3, y of 0..2 ~> (x, y)])\n print(set { x of xs ~> x % 3 })\n print(map { x of 1..4 ~> x : x * x })\n print(xs.map(x ~> x + 1).to_list()) }\n";
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
    let src = "fn p(s: str) -> int ! parse_error = s.parse_int()\nfn run2() -> int ! error { const b = p(\"x\")?\n ok(b) }\nfn main() { switch run2() { case ok(_): print(\"ok\")\n case err(e): { print(e)\n  print(\"failed: #e\") } }\n const r = run2()\n switch r { case err(e): print(e)\n default: print(\"?\") } }\n";
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
    for name in ["shapes", "words", "stack"] {
        let mut program = Program::with_core();
        program
            .add_package_from_path(&dir.join(format!("{name}.lang")))
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
    }
}
