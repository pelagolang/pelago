//! Type checker tests.

use super::*;
use crate::package::Program;
use crate::resolve::resolve;

struct Checked {
    program: Program,
    res: Resolved,
    typed: Typed,
}

fn run(src: &str) -> Checked {
    let mut program = Program::with_core();
    program.add_package("app", &[("main", src)]);
    let res = resolve(&program);
    let typed = check(&program, &res);
    Checked {
        program,
        res,
        typed,
    }
}

impl Checked {
    fn errors(&self) -> Vec<String> {
        let main = self
            .program
            .modules
            .iter()
            .find(|m| m.path == "main")
            .map(|m| m.id);
        self.typed
            .diagnostics
            .iter()
            .filter(|d| Some(d.module) == main || d.module.0 == 0 || true)
            .map(|d| d.message.clone())
            .collect()
    }

    /// The type of the first local, parameter, constant or field named `name` in `main`.
    fn ty_of(&self, name: &str) -> String {
        let main = self
            .program
            .modules
            .iter()
            .find(|m| m.path == "main")
            .map(|m| m.id);
        for (i, d) in self.res.defs.iter().enumerate() {
            if d.name == name && d.file == main {
                if let Some(t) = self.typed.local_tys.get(&DefId(i as u32)) {
                    return self.typed.show(*t);
                }
            }
        }
        format!("<no type for {name}>")
    }
}

fn clean(src: &str) {
    let c = run(src);
    assert!(
        c.errors().is_empty(),
        "unexpected errors in {src:?}: {:?}",
        c.errors()
    );
}

fn flagged(src: &str, needle: &str) {
    let c = run(src);
    let e = c.errors();
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "expected {needle:?} in {src:?}, got {e:?}"
    );
}

/// Wraps a statement list in a function and returns the type of `probe`.
fn probe(body: &str) -> String {
    let src = format!("fn t() {{\n{body}\n}}");
    let c = run(&src);
    assert!(
        c.errors().is_empty(),
        "unexpected errors in {src:?}: {:?}",
        c.errors()
    );
    c.ty_of("probe")
}

#[test]
fn the_core_library_checks_cleanly() {
    let program = Program::with_core();
    let res = resolve(&program);
    let typed = check(&program, &res);
    assert!(
        typed.diagnostics.is_empty(),
        "{:?}",
        typed
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}

#[test]
fn literals_default_and_adapt() {
    assert_eq!(probe("const probe = 5"), "int");
    assert_eq!(probe("const probe = 2.5"), "float");
    assert_eq!(probe("const probe = \"hi\""), "string");
    assert_eq!(probe("const probe = 'c'"), "char");
    assert_eq!(probe("const probe = true"), "bool");
    assert_eq!(probe("const probe: uint8 = 5"), "uint8");
    assert_eq!(probe("const probe: float32 = 1.5"), "float32");
    assert_eq!(probe("const probe: int? = 5"), "int?");
    assert_eq!(probe("const probe = -5"), "int");
    assert_eq!(probe("const probe: int8 = -128"), "int8");
}

#[test]
fn literals_must_fit() {
    flagged("fn t() { const x: uint8 = 300 }", "does not fit in `uint8`");
    flagged("fn t() { const x: int8 = 128 }", "does not fit in `int8`");
    flagged("fn t() { const x: uint8 = -1 }", "does not fit in `uint8`");
    flagged("fn t() { const x: float = 5 }", "is expected");
    clean("fn t() { const x: uint8 = 255\n const y: int8 = -128 }");
}

#[test]
fn a_literal_takes_its_type_from_later_use() {
    assert_eq!(
        probe("const a = 1\n const b: uint8 = 2\n const probe = a + b"),
        "uint8"
    );
    assert_eq!(
        probe("var probe = 0\n const b: int32 = 7\n probe = b"),
        "int32"
    );
}

#[test]
fn arithmetic_and_comparison() {
    assert_eq!(probe("const probe = 1 + 2 * 3"), "int");
    assert_eq!(probe("const probe = 1.5 * 2.0"), "float");
    assert_eq!(probe("const probe = 1 < 2"), "bool");
    assert_eq!(probe("const probe = \"a\" + \"b\""), "string");
    assert_eq!(probe("const probe = 7 % 3"), "int");
    assert_eq!(probe("const probe = 2 ** 10"), "int");
    assert_eq!(probe("const probe = 1 << 3"), "int");
    assert_eq!(probe("const a: uint8 = 1\n const probe = a +% 1"), "uint8");
    assert_eq!(probe("const probe = !true && false || true"), "bool");
    flagged("fn t() { const x = 1 + 2.0 }", "cannot use `+`");
    flagged("fn t() { const x = 1 + \"a\" }", "cannot use `+`");
    flagged("fn t() { const x = true + false }", "needs numbers");
    flagged("fn t() { const x = 1.5 & 2.5 }", "needs integers");
    flagged("fn t() { const x = 1 && 2 }", "is expected");
    flagged("fn t() { const x = !5 }", "is expected");
    flagged("fn t() { const x = 1 < \"a\" }", "cannot compare");
    flagged(
        "fn t() { const a: uint8 = 1\n const b: int = 2\n const x = a + b }",
        "convert with `to`",
    );
}

#[test]
fn conversions() {
    assert_eq!(probe("const probe = 7 to float"), "float");
    assert_eq!(probe("const a: int8 = 1\n const probe = a to int"), "int");
    assert_eq!(
        probe("const a: uint8 = 1\n const probe = a to int32"),
        "int32"
    );
    assert_eq!(probe("const probe = 5 to string"), "string");
    assert_eq!(probe("const probe = 'a' to int"), "int");
    flagged(
        "fn t() { const a = 2.5\n const x = a to int }",
        "lose information",
    );
    flagged(
        "fn t() { const a: int = 1\n const x = a to int8 }",
        "lose information",
    );
    flagged(
        "fn t() { const a = \"x\"\n const b = a to int }",
        "cannot convert",
    );
}

#[test]
fn variables_and_assignment() {
    clean("fn t() { var x = 1\n x = 2\n x += 3\n x++ }");
    flagged("fn t() { var x = 1\n x = \"a\" }", "is expected");
    flagged("fn t() { var s = \"a\"\n s -= \"b\" }", "does not work on");
    flagged("fn t() { var x = 1\n x += 2.5 }", "cannot use `+=`");
    clean("fn t() { var o: int? = none\n o ??= 3 }");
    flagged("fn t() { var o = 1\n o ??= 3 }", "needs an option");
}

#[test]
fn tuples_and_indexing() {
    assert_eq!(
        probe("const probe = (1, \"a\", true)"),
        "(int, string, bool)"
    );
    assert_eq!(probe("const t = (1, \"a\")\n const probe = t.1"), "string");
    assert_eq!(
        probe("const (a, b) = (1, \"x\")\n const probe = b"),
        "string"
    );
    flagged(
        "fn t() { const t = (1, 2)\n const x = t.5 }",
        "no position 5",
    );
    assert_eq!(probe("const xs = [1, 2, 3]\n const probe = xs[0]"), "int");
    assert_eq!(
        probe("const xs = [1, 2, 3]\n const probe = xs[1..]"),
        "[int]"
    );
    assert_eq!(
        probe("const m = map { \"a\": 1 }\n const probe = m[\"a\"]"),
        "int"
    );
    assert_eq!(probe("const s = \"hello\"\n const probe = s[0..2]"), "str");
    flagged(
        "fn t() { const s = \"hello\"\n const x = s[0] }",
        "cannot be indexed by position",
    );
    flagged(
        "fn t() { const xs = [1]\n const x = xs[\"a\"] }",
        "is expected",
    );
}

#[test]
fn collections() {
    assert_eq!(probe("const probe = [1, 2, 3]"), "[int]");
    assert_eq!(probe("const probe = [\"a\"]"), "[string]");
    assert_eq!(probe("const probe: [uint8] = [1, 2]"), "[uint8]");
    assert_eq!(probe("const probe: [int; 3] = [1, 2, 3]"), "[int; 3]");
    assert_eq!(probe("const probe = [0; 9]"), "[int; 9]");
    assert_eq!(probe("const probe: [int] = []"), "[int]");
    assert_eq!(
        probe("const probe = map { \"a\": 1, \"b\": 2 }"),
        "map<string, int>"
    );
    assert_eq!(
        probe("const probe: map<string, int> = {:}"),
        "map<string, int>"
    );
    assert_eq!(probe("const probe = set { 1, 2 }"), "set<int>");
    assert_eq!(
        probe("const xs = [1]\n const probe = [0, ...xs, 9]"),
        "[int]"
    );
    flagged("fn t() { const xs = [1, \"a\"] }", "is expected");
    flagged("fn t() { const xs = [] }", "cannot work out the type");
    flagged("fn t() { const x = none }", "cannot work out the type");
    flagged("fn t() { const xs: [int; 2] = [1, 2, 3] }", "3 items");
    flagged("fn t() { const m = {:} }", "cannot work out the type");
}

#[test]
fn builders() {
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = [x of xs if x > 0 ~> x * 2]"),
        "[int]"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = [x of xs ~> \"#x\"]"),
        "[string]"
    );
    assert_eq!(
        probe("const xs = [\"a\"]\n const probe = [i, x of xs ~> (i, x)]"),
        "[(int, string)]"
    );
    assert_eq!(
        probe("const xs = [1]\n const probe = set { x of xs ~> x % 2 }"),
        "set<int>"
    );
    assert_eq!(
        probe("const xs = [\"a\"]\n const probe = map { x of xs ~> x : 1 }"),
        "map<string, int>"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = [x of xs if x > 0]"),
        "[int]"
    );
}

#[test]
fn option_and_result() {
    assert_eq!(probe("const probe: int? = none"), "int?");
    assert_eq!(probe("const a: int? = 3\n const probe = a ?? 0"), "int");
    flagged(
        "fn t() { const a = 3\n const x = a ?? 0 }",
        "needs an option",
    );
    assert_eq!(probe("const probe: int ! error = ok(3)"), "int ! error");
    clean("fn f() -> int ! error { ok(1) }");
    clean("fn f() -> int ! error { err(error_value()) }\nfn error_value() -> error { todo() }");
    flagged("fn f() -> int ! error { ok(\"a\") }", "carries `string`");
    clean("fn parse(s: str) -> int ! parse_error { s.parse_int() }\nfn g(s: str) -> int ! error { const n = parse(s)?\n ok(n + 1) }");
    flagged("fn parse(s: str) -> int ! parse_error { s.parse_int() }\nfn g(s: str) -> int ! string { const n = parse(s)?\n ok(n) }", "would return an error");
    flagged("fn parse(s: str) -> int ! parse_error { s.parse_int() }\nfn g(s: str) -> int { parse(s)? }", "not a result");
    clean("fn first(xs: [int]) -> int? { const x = xs.first()?\n x }");
    flagged(
        "fn first(xs: [int]) -> int { xs.first()? }",
        "not an option",
    );
    flagged(
        "fn t() { const x = 5\n const y = x? }",
        "needs a result or an option",
    );
}

#[test]
fn optional_chaining() {
    clean("struct user { name: string }\nfn f(u: user?) -> string? { u?.name }");
    flagged(
        "struct user { name: string }\nfn f(u: user) -> string? { u?.name }",
        "needs an option",
    );
    assert_eq!(
        probe("const xs = [1]\n const probe = xs.first()?.abs()"),
        "int?"
    );
}

#[test]
fn functions_and_calls() {
    clean("fn add(a: int, b: int) -> int { a + b }\nfn t() { const x = add(1, 2) }");
    assert_eq!(
        probe("const probe = add(1, 2)\n fn add(a: int, b: int) -> int = a + b"),
        "int"
    );
    flagged(
        "fn add(a: int, b: int) -> int = a + b\nfn t() { add(1) }",
        "missing `b`",
    );
    flagged(
        "fn add(a: int, b: int) -> int = a + b\nfn t() { add(1, 2, 3) }",
        "takes 2 arguments",
    );
    flagged(
        "fn add(a: int, b: int) -> int = a + b\nfn t() { add(1, \"x\") }",
        "is expected",
    );
    clean("fn g(name: string, greeting: string = \"Hello\") {}\nfn t() { g(\"a\")\n g(\"a\", greeting: \"hi\")\n g(greeting: \"x\", name: \"y\") }");
    flagged(
        "fn g(name: string) {}\nfn t() { g(nme: \"a\") }",
        "no parameter named `nme`",
    );
    clean("fn sum(...nums: int) -> int { var t = 0\n for n of nums { t += n }\n t }\nfn t() { sum()\n sum(1, 2, 3)\n const xs = [1, 2]\n sum(...xs) }");
    flagged(
        "fn sum(...nums: int) -> int = 0\nfn t() { sum(1, \"a\") }",
        "is expected",
    );
}

#[test]
fn access_modes_are_repeated_at_the_call() {
    clean("fn clear(inout xs: [int]) { xs.clear!() }\nfn t() { var a = [1]\n clear(inout a) }");
    flagged(
        "fn clear(inout xs: [int]) { xs.clear!() }\nfn t() { var a = [1]\n clear(a) }",
        "write `inout ...`",
    );
    flagged(
        "fn show(xs: [int]) {}\nfn t() { var a = [1]\n show(inout a) }",
        "only reads",
    );
    clean("fn fin(sink xs: [int]) -> int = xs.len()\nfn t() { var a = [1]\n fin(sink a) }");
    flagged(
        "fn fin(sink xs: [int]) -> int = xs.len()\nfn t() { var a = [1]\n fin(a) }",
        "write `sink ...`",
    );
}

#[test]
fn generic_functions() {
    assert_eq!(
        probe("const probe = id(5)\n fn id<T>(x: T) -> T = x"),
        "int"
    );
    assert_eq!(
        probe("const probe = id(\"a\")\n fn id<T>(x: T) -> T = x"),
        "string"
    );
    assert_eq!(
        probe("const probe = id<uint8>(5)\n fn id<T>(x: T) -> T = x"),
        "uint8"
    );
    flagged(
        "fn pair<T>(a: T, b: T) -> T = a\nfn t() { pair(1, \"x\") }",
        "is expected",
    );
    assert_eq!(
        probe("const probe = first([1, 2])\n fn first<T>(xs: [T]) -> T = xs[0]"),
        "int"
    );
}

#[test]
fn return_and_diverging() {
    clean("fn f(x: int) -> int { if x > 0 { return 1 }\n 2 }");
    flagged("fn f(x: int) -> int { return \"a\" }", "is expected");
    flagged("fn f(x: int) -> int { return }", "needs a value");
    clean("fn f() -> int { panic(\"no\") }");
    clean("fn f(x: int) -> int { if x > 0 { return 1 } else { return 2 } }");
    flagged("fn f(x: int) -> int { if x > 0 { 1 } }", "no value");
    flagged("fn f() -> int { const x = 1 }", "is expected");
    clean("fn f() { return }");
}

#[test]
fn if_switch_and_ternary_agree_on_a_type() {
    assert_eq!(probe("const probe = true ? 1 : 2"), "int");
    flagged("fn t() { const x = true ? 1 : \"a\" }", "different types");
    assert_eq!(
        probe("const x = 3\n const probe = if x > 1 { 10 } else { 20 }"),
        "int"
    );
    flagged(
        "fn t() { const x = 3\n const y = if x > 1 { 10 } else { \"a\" } }",
        "different types",
    );
    clean("fn t() { if true { 1 } else { 2 } }");
    assert_eq!(probe("const d = 3\n const probe = switch d { case 1: \"a\"\n case 2: \"b\"\n default: \"c\" }"), "string");
    flagged(
        "fn t() { const d = 3\n const x = switch d { case 1: \"a\"\n default: 2 } }",
        "different types",
    );
}

#[test]
fn loops_and_breaks() {
    clean("fn t(xs: [int]) { for x of xs { x + 1 }\n for i, x of xs { i + x }\n for k in 0..3 { k } }");
    clean("fn t(m: map<string, int>) { for k, v of m { k.len() + v }\n for k in m { k.len() } }");
    assert_eq!(probe("const probe = loop { break 5 }"), "int");
    flagged(
        "fn t() { const x = loop { break 5\n } \n const y = while true { break 1 } }",
        "only `loop`",
    );
    flagged(
        "fn t() { const x = loop { if true { break 1 } else { break \"a\" } } }",
        "expected",
    );
    clean("fn t() { while true { break }\n do { 1 } while false }");
    flagged("fn t() { for x of \"abc\" {} }", "text is not a list");
    flagged("fn t() { for x of 5 {} }", "cannot loop over");
    clean("fn t() { var i = 0\n for (var j = 0; j < 3; j += 1) { i += j } }");
    assert_eq!(
        probe("const xs = [1, 2]\n var p = 0\n for x of xs { p = x }\n const probe = p"),
        "int"
    );
    clean("fn f() -> int { outer: loop { loop { break outer 7 } } }");
    clean("fn f() -> int { blk: { if true { break blk 1 }\n 2 } }");
}

#[test]
fn ranges() {
    assert_eq!(probe("const probe = 0..10"), "range<int>");
    assert_eq!(probe("const probe = 'a'..='z'"), "range<char>");
    assert_eq!(probe("const probe = 0..10 step 2"), "range<int>");
    assert_eq!(probe("const x = 5\n const probe = x in 1..10"), "bool");
    flagged("fn t() { const x = 1..\"a\" }", "same type");
    flagged("fn t() { const x = \"a\" in 1..10 }", "is expected");
}

#[test]
fn closures() {
    assert_eq!(
        probe("const probe = (x: int) -> int ~> x * 2"),
        "fn(int) -> int"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = xs.map(x ~> x * 2).to_list()"),
        "[int]"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = xs.map(~> it * 2).to_list()"),
        "[int]"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = xs.filter(~> it > 1).to_list()"),
        "[int]"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = xs.map(x ~> \"#x\").to_list()"),
        "[string]"
    );
    assert_eq!(
        probe("const xs = [1, 2]\n const probe = xs.map(x ~> x * 2).sum()"),
        "int"
    );
    flagged("fn t() { const f = x ~> x }", "cannot work out the type");
    clean("fn apply(f: fn(int) -> int, x: int) -> int = f(x)\nfn t() { apply(x ~> x + 1, 3) }");
    flagged(
        "fn apply(f: fn(int) -> int, x: int) -> int = f(x)\nfn t() { apply(x ~> \"a\", 3) }",
        "is expected",
    );
    flagged(
        "fn apply(f: fn(int) -> int, x: int) -> int = f(x)\nfn t() { apply((a, b) ~> a, 3) }",
        "takes 2 parameters",
    );
    clean("fn t() { var total = 0\n [1, 2].each([inout total] x ~> total += x) }");
    assert_eq!(
        probe(
            "const raw = \" 5 \"\n const probe = raw |> clean\n fn clean(s: str) -> str = s.trim()"
        ),
        "str"
    );
}

#[test]
fn strings_and_methods() {
    assert_eq!(probe("const probe = \"abc\".len()"), "int");
    assert_eq!(probe("const probe = \"abc\".chars().to_list()"), "[char]");
    assert_eq!(
        probe("const probe = \"a b\".split(\" \").to_list()"),
        "[str]"
    );
    assert_eq!(probe("const probe = \"x\".to_upper()"), "string");
    assert_eq!(
        probe("const probe = \"5\".parse_int()"),
        "int ! parse_error"
    );
    assert_eq!(probe("const probe = \"v: #(1 + 2)\""), "string");
    flagged("fn t() { \"abc\".nope() }", "has no method `nope`");
    flagged("fn t() { \"abc\".lenn() }", "did you mean `len`");
}

#[test]
fn method_marks() {
    clean("fn t() { var xs = [1]\n xs.push!(2)\n xs.clear!() }");
    flagged(
        "fn t() { var xs = [1]\n xs.push(2) }",
        "write `.push!(...)`",
    );
    flagged("fn t() { var xs = [1]\n xs.len!() }", "only reads");
    clean("struct c { n: int\n fn bump(inout self) { self.n++ }\n fn close(sink self) -> int = self.n\n fn show(self) -> int = self.n }\nfn t() { var a = c { n: 0 }\n a.bump!()\n a.show()\n a.close!!() }");
    flagged("struct c { n: int\n fn close(sink self) -> int = self.n }\nfn t() { var a = c { n: 0 }\n a.close() }", "write `.close!!(...)`");
    flagged("struct c { n: int\n fn close(sink self) -> int = self.n }\nfn t() { var a = c { n: 0 }\n a.close!() }", "consumes");
}

#[test]
fn structs() {
    clean("struct point { x: float\n y: float }\nfn t() { const p = point { x: 1.0, y: 2.0 }\n const a = p.x + p.y }");
    assert_eq!(probe("const p = point { x: 1.0, y: 2.0 }\n const probe = p.x\n struct point { x: float\n y: float }"), "float");
    flagged(
        "struct point { x: float\n y: float }\nfn t() { point { x: 1.0 } }",
        "needs `y`",
    );
    flagged(
        "struct point { x: float\n y: float }\nfn t() { point { x: 1.0, y: 2.0, z: 3.0 } }",
        "no field `z`",
    );
    flagged(
        "struct point { x: float }\nfn t() { point { x: \"a\" } }",
        "is expected",
    );
    flagged(
        "struct point { x: float }\nfn t(p: point) { p.y }",
        "no field or method `y`",
    );
    flagged(
        "struct point { width: float }\nfn t(p: point) { p.widht }",
        "did you mean `width`",
    );
    clean("struct config { port: int = 8080\n verbose: bool = false }\nfn t() { config {} }");
    clean("struct p { x: int }\nfn t(a: p) { const b = p { ...a } }");
    clean("struct p { x: int\n y: int }\nfn t(x: int, y: int) { p { x, y } }");
    clean("struct p { x: int\n fn len(self) -> int = self.x\n fn zero() -> p = p { x: 0 } }\nfn t() { const a = p.zero()\n a.len() }");
}

#[test]
fn private_fields() {
    let mut program = Program::with_core();
    program.add_package(
        "lib",
        &[(
            "shapes",
            "pub struct s { x: int\n pub y: int }\npub fn make() -> s = s { x: 1, y: 2 }",
        )],
    );
    program.add_package(
        "app",
        &[(
            "main",
            "import lib.shapes\nfn t() { const a = shapes.make()\n a.y\n a.x }",
        )],
    );
    let res = resolve(&program);
    let typed = check(&program, &res);
    let msgs: Vec<String> = typed
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("`x` of `s` is private")),
        "{msgs:?}"
    );
    assert!(!msgs.iter().any(|m| m.contains("`y`")), "{msgs:?}");
}

#[test]
fn private_fields_block_the_literal() {
    let mut program = Program::with_core();
    program.add_package("lib", &[("shapes", "pub struct s { x: int\n pub y: int }")]);
    program.add_package(
        "app",
        &[(
            "main",
            "import lib.shapes\nfn t() { shapes.s { x: 1, y: 2 } }",
        )],
    );
    let res = resolve(&program);
    let typed = check(&program, &res);
    let msgs: Vec<String> = typed
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("private fields")),
        "{msgs:?}"
    );
}

#[test]
fn enums_and_patterns() {
    let shape = "enum shape { circle(radius: float)\n rect(w: float, h: float)\n empty }\n";
    clean(&format!("{shape}fn area(s: shape) -> float {{ switch s {{ case .circle(r): r * r\n case .rect(w, h): w * h\n case .empty: 0.0 }} }}"));
    clean(&format!("{shape}fn t() {{ const a = shape.circle(radius: 2.0)\n const b: shape = .rect(w: 1.0, h: 2.0)\n const c: shape = .empty }}"));
    assert_eq!(
        probe(&format!("{shape}const probe = shape.circle(radius: 2.0)").replace("fn", "fn"))
            .replace(" ", ""),
        "shape"
    );
    flagged(
        &format!("{shape}fn t(s: shape) {{ switch s {{ case .square(x): 1\n default: 0 }} }}"),
        "no case `square`",
    );
    flagged(
        &format!("{shape}fn t(s: shape) {{ switch s {{ case .circle(a, b): 1\n default: 0 }} }}"),
        "carries 1 value",
    );
    flagged(
        &format!("{shape}fn t(s: shape) {{ switch s {{ case .circle: 1\n default: 0 }} }}"),
        "carries 1 value",
    );
    flagged(
        &format!("{shape}fn t() {{ shape.circle(radius: \"a\") }}"),
        "is expected",
    );
    flagged(
        &format!("{shape}fn t() {{ const x = .circle(radius: 1.0) }}"),
        "cannot tell which enum",
    );
    clean(&format!("{shape}fn t(s: shape) {{ if s is .circle(r) {{ r + 1.0 }}\n guard s is .rect(w, h) else {{ return }}\n w + h }}"));
    clean(&format!("{shape}fn t(s: shape) -> float {{ switch s {{ case .rect(w: width, h): width * h\n default: 0.0 }} }}"));
}

#[test]
fn generic_enums_and_structs() {
    clean("enum tree<T> { leaf(T)\n node(box<tree<T>>, box<tree<T>>) }\nfn t() { const a: tree<int> = .leaf(1) }");
    clean("struct stack<T> { items: [T]\n fn size(self) -> int = self.items.len() }\nfn t() { const s = stack<int> { items: [1] }\n s.size() }");
    assert_eq!(probe("const s = stack { items: [1] }\n const probe = s.items\n struct stack<T> { items: [T] }"), "[int]");
}

#[test]
fn option_and_result_patterns() {
    clean("fn t(o: int?) -> int { switch o { case some(v): v\n case none: 0 } }");
    clean("fn t(r: int ! error) -> int { switch r { case ok(v): v\n case err(e): 0 } }");
    flagged(
        "fn t(o: int) { switch o { case some(v): v\n default: 0 } }",
        "pattern for an option",
    );
    flagged(
        "fn t(o: int?) { switch o { case ok(v): v\n default: 0 } }",
        "pattern for a result",
    );
    clean("fn t(o: int?) { if o is some(v) && v > 3 { v } }");
}

#[test]
fn list_patterns() {
    clean(
        "fn t(xs: [int]) -> int { switch xs { case []: 0\n case [h, ...rest]: h + rest.len()\n } }",
    );
    assert_eq!(
        probe("const xs = [1]\n switch xs { case [h, ...t]: h\n default: 0 }\n const probe = 0"),
        "int"
    );
    flagged(
        "fn t(x: int) { switch x { case [a]: a\n default: 0 } }",
        "needs a list",
    );
}

#[test]
fn tuple_and_literal_patterns() {
    clean("fn t(p: (int, int)) -> int { switch p { case (0, 0): 0\n case (x, 0): x\n case (_, y): y } }");
    flagged(
        "fn t(p: (int, int)) { switch p { case (a, b, c): a\n default: 0 } }",
        "3 parts",
    );
    flagged(
        "fn t(x: int) { switch x { case \"a\": 1\n default: 0 } }",
        "this pattern is",
    );
    clean("fn t(x: int) -> string { switch x { case 0..10: \"small\"\n case n if n > 100: \"big\"\n default: \"mid\" } }");
    clean("fn t(s: string) { switch s { case \"a\": 1\n default: 0 } }");
}

#[test]
fn struct_patterns() {
    clean("struct point { x: int\n y: int }\nfn t(p: point) -> int { switch p { case point { x: 0, y }: y\n case point { x, .. }: x } }");
    flagged("struct point { x: int\n y: int }\nfn t(p: point) { switch p { case point { x }: x\n default: 0 } }", "leaves out `y`");
}

#[test]
fn objects_and_inheritance() {
    let src = "open obj animal { name: string\n open fn speak(self) -> string = \"...\" }\nobj dog : animal { override fn speak(self) -> string = \"Woof\"\n fn fetch(self) -> int = 1 }\n";
    clean(&format!(
        "{src}fn t() {{ const d: animal = dog {{ name: \"rex\" }}\n d.speak() }}"
    ));
    clean(&format!(
        "{src}fn t(d: dog) {{ d.name\n d.speak()\n d.fetch() }}"
    ));
    clean(&format!(
        "{src}fn feed(a: animal) {{}}\nfn t(d: dog) {{ feed(d) }}"
    ));
    flagged(
        &format!("{src}fn feed(d: dog) {{}}\nfn t(a: animal) {{ feed(a) }}"),
        "is expected",
    );
    clean(&format!(
        "{src}fn t(a: animal) {{ const m = a to? dog\n if a is dog {{ 1 }} }}"
    ));
    flagged(
        &format!("{src}struct other {{}}\nfn t(a: animal) {{ a to? other }}"),
        "not a kind of",
    );
    clean(&format!(
        "{src}fn t(a: animal) {{ switch a {{ case d is dog: d.fetch()\n default: 0 }} }}"
    ));
}

#[test]
fn init_constructors() {
    clean("struct range2 { lo: int\n hi: int\n init(lo: int, hi: int) -> Self ! error { if lo > hi { return err(todo()) }\n self.lo = lo\n self.hi = hi } }\nfn t() { const r = range2(lo: 1, hi: 2) }");
    clean("struct counter { n: int\n init(start: int) { self.n = start } }\nfn t() { const c = counter(start: 3) }");
    flagged(
        "struct counter { n: int }\nfn t() { const c = counter(3) }",
        "has no `init`",
    );
    flagged("struct counter { n: int\n init(start: int) { self.n = start } }\nfn t() { counter(\"a\") }", "is expected");
}

#[test]
fn constants_and_globals() {
    clean("const limit = 10\nfn t() { const x = limit + 1 }");
    assert_eq!(
        run("const limit = 3.5\nfn t() { const probe = limit }").ty_of("probe"),
        "float"
    );
    flagged("const a = b\nconst b = a", "depends on itself");
    clean("const names: [string] = [\"a\"]\nfn t() { names.len() }");
}

#[test]
fn interfaces_and_impls() {
    clean("interface greeter { fn greet(self) -> string }\nstruct bot { n: int }\nimpl greeter for bot { fn greet(self) -> string = \"hi\" }\nfn t(b: bot) { b.greet() }");
    clean("struct v { x: int }\nimpl v { fn double(self) -> int = self.x * 2 }\nfn t(a: v) { a.double() }");
    flagged("fn t(x: int) { x.nothing() }", "has no method `nothing`");
}

#[test]
fn nested_functions_and_local_types() {
    clean("fn t() { const a = helper(2)\n fn helper(x: int) -> int = x * 2 }");
    clean("fn t() { struct p { x: int }\n const a = p { x: 1 }\n a.x }");
}

#[test]
fn deferred_literal_types_resolve_across_statements() {
    assert_eq!(
        probe("var probe = 0\n const small: uint8 = 3\n probe = small"),
        "uint8"
    );
}

#[test]
fn main_and_entry_forms() {
    clean("fn main() { print(\"hi\") }");
    clean("fn main(args: [string]) { print(args.len()) }");
    clean("fn main() -> void ! error { print(1) }");
}

#[test]
fn print_accepts_anything() {
    clean("fn t() { print(1, \"a\", true, [1, 2]) }");
}

#[test]
fn checking_never_panics_on_damaged_input() {
    let tour =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/tour.pel"))
            .unwrap();
    let chars: Vec<char> = tour.chars().collect();
    let mut state = 0xABCD_EF01_2345_6789u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    for _ in 0..150 {
        let mut c = chars.clone();
        for _ in 0..1 + next(6) {
            let i = next(c.len());
            match next(3) {
                0 => {
                    c.remove(i);
                }
                1 => c.insert(
                    i,
                    ['{', '}', '(', ')', ':', '.', 'x', '\n', '<', '>'][next(10)],
                ),
                _ => {
                    let j = next(c.len());
                    c.swap(i, j);
                }
            }
        }
        let src: String = c.into_iter().collect();
        let mut program = Program::with_core();
        program.add_package("app", &[("main", src.as_str())]);
        let res = resolve(&program);
        let _ = check(&program, &res);
    }
}

#[test]
fn the_tour_checks_without_panicking() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/tour.pel"))
        .unwrap();
    let mut program = Program::with_core();
    program.add_package("app", &[("tour", src.as_str())]);
    let res = resolve(&program);
    let typed = check(&program, &res);
    assert!(!typed.tys.is_empty());
}

// ---- Stage 3: interfaces ----------------------------------------------------------------------

#[test]
fn bounds_are_checked_at_calls() {
    let defs = "interface shape { fn area(self) -> float }\nstruct sq { s: float }\nimpl shape for sq { fn area(self) -> float = self.s * self.s }\nstruct blob { n: int }\nfn total<T: shape>(x: T) -> float = x.area()\n";
    clean(&format!("{defs}fn t() {{ total(sq {{ s: 2.0 }}) }}"));
    flagged(
        &format!("{defs}fn t() {{ total(blob {{ n: 1 }}) }}"),
        "`blob` does not provide `shape`, which the type parameter `T` needs; write `impl shape for blob`",
    );
    flagged(
        "fn big<T: order>(a: T, b: T) -> bool = a > b\nobj thing { n: int }\nfn t(a: thing, b: thing) { big(a, b) }",
        "does not provide `order`",
    );
    clean("fn big<T: order>(a: T, b: T) -> bool = a > b\nfn t() { big(1, 2) }");
    clean("fn big<T: order>(a: T, b: T) -> bool = a > b\nfn t() { big(\"a\", \"b\") }");
    clean("fn big<T>(a: T) -> T where T: show = a\nfn t() { big(1) }");
    flagged(
        "fn big<T: eq>(a: T) -> T = a\nfn t() { big(print) }",
        "does not provide `eq`",
    );
}

#[test]
fn bounds_on_type_arguments() {
    let defs = "struct bag<T: hash> { items: [T] }\n";
    clean(&format!("{defs}fn t(b: bag<int>) {{ }}"));
    flagged(
        &format!("{defs}struct k {{ n: int }}\nfn t(b: bag<k>) {{ }}"),
        "does not provide `hash`",
    );
    clean(&format!(
        "{defs}struct k {{ n: int }}\n@derive(hash)\nstruct j {{ n: int }}\nfn t(b: bag<j>) {{ }}"
    ));
}

#[test]
fn methods_through_bounds_and_defaults() {
    clean("interface named { fn name(self) -> string\n fn shout(self) -> string = self.name() + \"!\" }\nstruct d { n: string }\nimpl named for d { fn name(self) -> string = self.n }\nfn t(x: d) -> string = x.shout()");
    clean("interface named { fn name(self) -> string }\nfn t<T: named>(x: T) -> string = x.name()");
    flagged(
        "interface named { fn name(self) -> string }\nfn t<T>(x: T) -> string = x.name()",
        "has no method `name`",
    );
    clean("interface named { fn name(self) -> string }\nfn t(x: any named) -> string = x.name()");
    clean("interface named { fn name(self) -> string }\ninterface titled : named { fn title(self) -> string }\nfn t<T: titled>(x: T) -> string = x.name()");
}

#[test]
fn impls_must_be_complete_and_match() {
    flagged(
        "interface g { fn greet(self) -> string }\nstruct b { n: int }\nimpl g for b { }",
        "`b` does not implement `greet`, which `g` promises",
    );
    flagged(
        "interface g { fn greet(self) -> string }\nstruct b { n: int }\nimpl g for b { fn greet(self) -> int = 1 }",
        "`greet` of `b` does not match `g`",
    );
    flagged(
        "interface g { fn greet(self, loud: bool) -> string }\nstruct b { n: int }\nimpl g for b { fn greet(self) -> string = \"\" }",
        "does not match `g`",
    );
    flagged(
        "interface g { fn greet(self) -> string }\nstruct b : g { n: int }",
        "does not implement `greet`",
    );
    clean("interface g { fn greet(self) -> string }\nstruct b : g { n: int\n fn greet(self) -> string = \"x\" }");
    clean(
        "interface g { fn greet(self) -> string = \"hi\" }\nstruct b { n: int }\nimpl g for b { }",
    );
    flagged(
        "interface a { fn x(self) -> int }\ninterface b : a { fn y(self) -> int }\nstruct s { n: int }\nimpl b for s { fn y(self) -> int = 1 }",
        "`b` builds on `a`, so `s` must provide `a` too",
    );
    flagged(
        "struct s { n: int }\nstruct u { n: int }\nimpl s for u { }",
        "this is not an interface",
    );
}

#[test]
fn operators_on_user_types() {
    let vec2 = "struct vec2 { x: float, y: float }\n";
    flagged(
        &format!("{vec2}fn t(a: vec2, b: vec2) {{ a + b }}"),
        "`+` does not work on `vec2`: it does not provide `add`; write `impl add for vec2`",
    );
    let add = "impl add for vec2 { fn add(self, other: vec2) -> vec2 = vec2 { x: self.x + other.x, y: self.y + other.y } }\n";
    clean(&format!(
        "{vec2}{add}fn t(a: vec2, b: vec2) -> vec2 = a + b"
    ));
    clean(&format!(
        "{vec2}{add}fn t(a: vec2, b: vec2) {{ var c = a\n c += b }}"
    ));
    flagged(
        &format!("{vec2}{add}fn t(a: vec2) {{ a - a }}"),
        "does not provide `sub`",
    );
    flagged(
        &format!("{vec2}{add}fn t(a: vec2) {{ -a }}"),
        "cannot negate `vec2`",
    );
    flagged(&format!("{vec2}{add}fn t(a: vec2) {{ a + 1 }}"), "expected");
    clean("fn sum<T: add>(a: T, b: T) -> T = a + b\nfn t() { sum(1, 2) }");
    flagged(
        "fn sum<T>(a: T, b: T) -> T = a + b",
        "does not provide `add`",
    );
}

#[test]
fn equality_and_order_need_interfaces() {
    clean("struct p { x: int }\nfn t(a: p, b: p) -> bool = a == b");
    flagged(
        "struct p { x: int }\nfn t(a: p, b: p) -> bool = a < b",
        "does not provide `order`",
    );
    clean("@derive(order)\nstruct p { x: int }\nfn t(a: p, b: p) -> bool = a < b");
    flagged(
        "obj o { x: int }\nfn t(a: o, b: o) -> bool = a == b",
        "does not provide `eq`",
    );
    clean("obj o { x: int }\nimpl eq for o { fn eq(self, other: o) -> bool = self.x == other.x }\nfn t(a: o, b: o) -> bool = a == b");
    clean("obj o { x: int }\nfn t(a: o?) -> bool = a == none");
    flagged(
        "fn t<T>(a: T, b: T) -> bool = a == b",
        "does not provide `eq`",
    );
    clean("fn t<T: eq>(a: T, b: T) -> bool = a == b");
    clean("fn t(a: [int], b: [int]) -> bool = a == b");
    flagged(
        "obj o { x: int }\nfn t(a: [o], b: [o]) -> bool = a == b",
        "does not provide `eq`",
    );
    flagged(
        "struct p { x: int, o: opaque }\nobj opaque { }\nfn t(a: p, b: p) -> bool = a == b",
        "does not provide `eq`",
    );
}

#[test]
fn any_needs_the_interface() {
    let defs = "interface shape { fn area(self) -> float }\nstruct sq { s: float }\nimpl shape for sq { fn area(self) -> float = 1.0 }\nstruct blob { n: int }\n";
    clean(&format!(
        "{defs}fn t(x: any shape) {{ }}\nfn u() {{ t(sq {{ s: 1.0 }}) }}"
    ));
    flagged(
        &format!("{defs}fn t(x: any shape) {{ }}\nfn u() {{ t(blob {{ n: 1 }}) }}"),
        "`blob` does not provide `shape`",
    );
    flagged(
        "struct blob { n: int }\nfn u() { print(blob { n: 1 }) }",
        "does not provide `show`",
    );
    clean("@derive(show)\nstruct blob { n: int }\nfn u() { print(blob { n: 1 }) }");
}

#[test]
fn the_orphan_rule() {
    // In another package, neither the interface nor the type belongs to us.
    let mut program = Program::with_core();
    program.add_package("app", &[("main", "fn t() { }\nimpl show for int { fn show(self) -> string = \"\" }\nimpl<T> list<T> { fn second(self) -> T? = self.get(1) }")]);
    let res = resolve(&program);
    let typed = check(&program, &res);
    let msgs: Vec<String> = typed
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("neither `show`, `int`")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("cannot add methods to `[T]` here")),
        "{msgs:?}"
    );
    // Our own type is fine.
    clean("struct mine { n: int }\nimpl show for mine { fn show(self) -> string = \"m\" }\nimpl<T> list<T> { }".replace("impl<T> list<T> { }", "").as_str());
}

#[test]
fn library_methods_carry_bounds() {
    clean("fn t(xs: [int]) { xs.contains(2)\n xs.sorted() }");
    flagged(
        "obj o { x: int }\nfn t(xs: [o], y: o) { xs.contains(y) }",
        "does not provide `eq`",
    );
    flagged(
        "obj o { x: int }\nfn t(xs: [o]) { xs.sorted() }",
        "does not provide `order`",
    );
}

#[test]
fn copy_needs_a_copyable_type() {
    clean("fn t(xs: [int]) { const b = copy xs }");
    flagged(
        "obj o { x: int }\nfn t(a: o) { const b = copy a }",
        "cannot",
    );
    clean("obj o { x: int }\nimpl clone for o { fn clone(self) -> o = o { x: self.x } }\nfn t(a: o) { const b = copy a }");
}

// ---- Stage 4: exhaustiveness --------------------------------------------------------------------

const SHAPES: &str = "enum shape { circle(float)\n rect(float, float)\n empty }\n";

#[test]
fn switch_must_cover_every_case() {
    clean(&format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r): 1\n case .rect(w, h): 2\n case .empty: 3 }} }}"));
    flagged(
        &format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r): 1\n case .empty: 3 }} }}"),
        "does not handle `.rect(_, _)`",
    );
    clean(&format!(
        "{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r): 1\n default: 0 }} }}"
    ));
    clean(&format!(
        "{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r): 1\n case other: 0 }} }}"
    ));
    // A guard may fail, so it does not count.
    flagged(
        &format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r) if r > 1.0: 1\n case .rect(w, h): 2\n case .empty: 3 }} }}"),
        "does not handle `.circle(_)`",
    );
    clean(&format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r) if r > 1.0: 1\n case .circle(r): 4\n case .rect(w, h): 2\n case .empty: 3 }} }}"));
    // Alternatives in one arm.
    clean(&format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(_), .rect(_, _): 1\n case .empty: 3 }} }}"));
}

#[test]
fn switch_over_options_results_bools_tuples() {
    flagged(
        "fn t(o: int?) -> int { switch o { case some(v): v } }",
        "does not handle `none`",
    );
    clean("fn t(o: int?) -> int { switch o { case some(v): v\n case none: 0 } }");
    flagged(
        "fn t(r: int ! error) -> int { switch r { case ok(v): v } }",
        "`err(_)`",
    );
    flagged(
        "fn t(b: bool) -> int { switch b { case true: 1 } }",
        "`false`",
    );
    clean("fn t(b: bool) -> int { switch b { case true: 1\n case false: 0 } }");
    flagged("fn t(a: bool, b: bool) -> int { switch (a, b) { case (true, true): 1\n case (false, _): 0 } }", "`(true, false)`");
    clean("fn t(a: bool, b: bool) -> int { switch (a, b) { case (true, _): 1\n case (false, _): 0 } }");
    flagged(
        "fn t(o: (int?)?) -> int { switch o { case some(some(v)): v\n case none: 0 } }",
        "`some(none)`",
    );
}

#[test]
fn numbers_and_text_need_a_default() {
    flagged(
        "fn t(n: int) -> int { switch n { case 0: 1\n case 1: 2 } }",
        "does not handle `_`",
    );
    clean("fn t(n: int) -> int { switch n { case 0: 1\n default: 2 } }");
    flagged(
        "fn t(s: string) -> int { switch s { case \"a\": 1 } }",
        "does not handle",
    );
    clean("fn t(n: int) -> int { switch n { case 0..10: 1\n case x: 2 } }");
}

#[test]
fn list_patterns_are_checked_by_length() {
    flagged(
        "fn t(xs: [int]) -> int { switch xs { case []: 0\n case [a]: 1 } }",
        "does not handle `[_, _, ...]`",
    );
    clean("fn t(xs: [int]) -> int { switch xs { case []: 0\n case [first, ...rest]: 1 } }");
    clean("fn t(xs: [int]) -> int { switch xs { case [..., last]: 0\n case []: 1 } }");
    flagged(
        "fn t(xs: [int]) -> int { switch xs { case [first, ...rest]: 1 } }",
        "does not handle `[]`",
    );
    clean("fn t(xs: [int]) -> int { switch xs { case []: 0\n case [a]: 1\n case [a, b, ...rest]: 2 } }");
}

#[test]
fn irrefutable_patterns_must_always_match() {
    clean("fn t(p: (int, int)) { const (a, b) = p }");
    clean("struct pt { x: int, y: int }\nfn t(p: pt) { const pt { x, y } = p }");
    flagged(
        &format!("{SHAPES}fn t(s: shape) {{ const .circle(r) = s }}"),
        "does not always match",
    );
    flagged(
        "fn t(o: int?) { const some(v) = o }",
        "does not always match",
    );
    flagged(
        "fn t(xs: [int]) { const [a, b] = xs }",
        "does not always match",
    );
    clean("fn t(xs: [(int, int)]) { for (a, b) of xs { } }");
    clean("enum one { only(int) }\nfn t(x: one) { const .only(n) = x }");
}

#[test]
fn recursive_enums_do_not_blow_up() {
    clean("enum tree { leaf\n node(box<tree>, box<tree>) }\nfn t(x: tree) -> int { switch x { case .leaf: 0\n case .node(_, _): 2 } }");
    flagged(
        "enum tree { leaf\n node(box<tree>, box<tree>) }\nfn t(x: tree) -> int { switch x { case .leaf: 0 } }",
        "does not handle `.node(_, _)`",
    );
}

// ---- Stage 6: errors, entry points, defer ---------------------------------------------------------

const APP_ERR: &str = "enum app_error { parse(parse_error)\n other }\nimpl converts<app_error> for parse_error { fn convert(self) -> app_error = .parse(self) }\n";

#[test]
fn question_mark_converts_errors() {
    clean(&format!(
        "{APP_ERR}fn t(s: str) -> int ! app_error {{ const n = s.parse_int()?\n ok(n) }}"
    ));
    flagged(
        "enum app_error { other }\nfn t(s: str) -> int ! app_error { const n = s.parse_int()?\n ok(n) }",
        "write `impl converts<app_error> for parse_error`",
    );
    clean("fn t(s: str) -> int ! error { const n = s.parse_int()?\n ok(n) }");
    clean(&format!(
        "{APP_ERR}fn t(e: parse_error) -> app_error = e to app_error"
    ));
    flagged(
        "enum app_error { other }\nfn t(e: parse_error) -> app_error = e to app_error",
        "cannot convert",
    );
    flagged(
        "enum app_error { other }\nstruct b { n: int }\nimpl converts<app_error> for b { fn convert(self) -> int = 1 }",
        "does not match `converts`",
    );
}

#[test]
fn main_has_fixed_forms() {
    clean("fn main() { }");
    clean("fn main(args: [string]) { }");
    clean("fn main() -> void ! error { }");
    flagged("fn main(a: int) { }", "`main` takes no parameters");
    flagged(
        "fn main(a: [string], b: int) { }",
        "`main` takes no parameters",
    );
    flagged("fn main() -> int { return 1 }", "`main` returns nothing");
    flagged("fn main<T>() { }", "cannot be generic");
}

#[test]
fn defer_cannot_leave_the_scope() {
    clean("fn t() { defer print(1) }");
    flagged(
        "fn t() -> int ! error { defer { return ok(1) }\n ok(2) }",
        "cannot use `return`",
    );
    flagged(
        "fn t() { for i of 0..2 { defer { break } } }",
        "cannot use `break`",
    );
    flagged(
        "fn t(s: str) -> void ! error { defer { s.parse_int()? } }",
        "cannot use `?`",
    );
    flagged(
        "fn f() -> void ! error { }\nfn t() { defer f() }",
        "returns a result",
    );
}

#[test]
fn overflow_operators_are_for_integers() {
    clean("fn t(a: uint8, b: uint8) -> uint8 = a +% b");
    clean("fn t(inout a: uint8) { a +%= 1\n a -%= 1\n a *%= 2 }");
    flagged(
        "fn t(a: float, b: float) -> float = a +% b",
        "needs integers",
    );
}

#[test]
fn unreachable_switch_arms_are_reported() {
    flagged(&format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r): 1\n case .circle(q): 2\n default: 0 }} }}"), "can never match");
    flagged(&format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r): 1\n case .rect(w, h): 2\n case .empty: 3\n default: 0 }} }}"), "can never match");
    flagged(
        "fn t(n: int) -> int { switch n { case x: 1\n case 5: 2 } }",
        "can never match",
    );
    clean(&format!("{SHAPES}fn t(s: shape) -> int {{ switch s {{ case .circle(r) if r > 1.0: 1\n case .circle(r): 2\n default: 0 }} }}"));
    clean("fn t(xs: [int]) -> int { switch xs { case []: 0\n case [a]: 1\n case [a, ...rest]: 2\n default: 3 } }".replace("\n default: 3", "").as_str());
    flagged("fn t(xs: [int]) -> int { switch xs { case [first, ...rest]: 1\n case [a]: 2\n default: 0 } }", "can never match");
    flagged(
        "fn t(o: int?) -> int { switch o { case some(v): v\n case none: 0\n case some(w): 1 } }",
        "can never match",
    );
}

#[test]
fn sink_marks_only_for_owning_values() {
    clean("fn take(sink n: int) { }\nfn t() { const a = 1\n take(a) }");
    flagged(
        "fn take(sink n: string) { }\nfn t() { const a = \"x\"\n take(a) }",
        "write `sink ...`",
    );
}

#[test]
fn channels_can_be_looped_over() {
    clean("fn t() { const (tx, rx) = chan<int>()\n for x of rx { print(x) } }");
}

// ---- Boxes -------------------------------------------------------------------------------------

#[test]
fn box_types_and_methods() {
    let c = run("struct pt { x: int, y: int }\nfn main() { const a = box(5)\n const b: box<int> = box(5)\n var p = box(pt { x: 1, y: 2 })\n const x = p.x\n const g = p.get()\n p.set!(pt { x: 3, y: 4 })\n const t = p.take!!() }");
    assert_eq!(c.errors(), Vec::<String>::new());
    assert_eq!(c.ty_of("a"), "box<int>");
    assert_eq!(c.ty_of("b"), "box<int>");
    assert_eq!(c.ty_of("x"), "int");
    assert_eq!(c.ty_of("g"), "pt");
    assert_eq!(c.ty_of("t"), "pt");
}

#[test]
fn box_needs_one_argument() {
    assert!(!run("fn main() { const a = box() }").errors().is_empty());
}

#[test]
fn box_forwards_indexing_and_methods() {
    clean("fn main() { const b = box([1, 2, 3])\n const n = b[0] + b.len() }");
}

#[test]
fn recursive_types_need_a_box() {
    clean("enum tree { leaf, node(box<tree>, int, box<tree>) }\nstruct list_of { next: box<list_of>?, v: int }\nstruct many { kids: [many] }\nfn main() {}");
    for src in [
        "struct a { x: a }\nfn main() {}",
        "struct a { x: a? }\nfn main() {}",
        "struct a { b: b }\nstruct b { a: a }\nfn main() {}",
        "enum e { one(e) }\nfn main() {}",
    ] {
        let errs = run(src).errors();
        assert!(
            errs.iter().any(|m| m.contains("infinitely large")),
            "{src}: {errs:?}"
        );
    }
}
