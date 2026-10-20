//! Parser tests: precedence, spacing rules, newlines, patterns, errors, and robustness.

use super::*;
use crate::ast::*;
use crate::ast_print::{print_file, Printer};

/// Parses `src`, asserts there are no errors, and prints the first statement on one line.
fn one(src: &str) -> String {
    let (out, lex) = parse_source(src);
    assert!(lex.is_empty(), "lex errors in {src:?}: {lex:?}");
    assert!(out.errors.is_empty(), "errors in {src:?}: {:?}", out.errors);
    let stmt = out.file.stmts.first().expect("a statement");
    Printer { src }.stmt(stmt).to_flat()
}

/// All statements, one per element.
fn all(src: &str) -> Vec<String> {
    let (out, lex) = parse_source(src);
    assert!(lex.is_empty(), "lex errors in {src:?}: {lex:?}");
    assert!(out.errors.is_empty(), "errors in {src:?}: {:?}", out.errors);
    let p = Printer { src };
    out.file.stmts.iter().map(|s| p.stmt(s).to_flat()).collect()
}

fn errors(src: &str) -> Vec<String> {
    let (out, _) = parse_source(src);
    out.errors.into_iter().map(|e| e.message).collect()
}

#[test]
fn arithmetic_precedence() {
    assert_eq!(one("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(one("(1 + 2) * 3"), "(* (paren (+ 1 2)) 3)");
    assert_eq!(one("a - b - c"), "(- (- a b) c)");
    assert_eq!(one("a * b % c"), "(% (* a b) c)");
    assert_eq!(one("a +% b *% c"), "(+% a (*% b c))");
    assert_eq!(one("a << 1 + 2"), "(<< a (+ 1 2))");
}

#[test]
fn bitwise_binds_tighter_than_comparison() {
    assert_eq!(one("a & b == c"), "(== (& a b) c)");
    assert_eq!(one("a | b ^ c & d"), "(| a (^ b (& c d)))");
}

#[test]
fn logical_and_comparison() {
    assert_eq!(one("a || b && c"), "(|| a (&& b c))");
    assert_eq!(one("0 < x && x < 10"), "(&& (< 0 x) (< x 10))");
    assert_eq!(one("a == b || !c"), "(|| (== a b) (not c))");
}

#[test]
fn power_is_right_associative_and_tighter_than_unary() {
    assert_eq!(one("2 ** 3 ** 2"), "(** 2 (** 3 2))");
    assert_eq!(one("-2 ** 2"), "(neg (** 2 2))");
    assert_eq!(one("!!x"), "(not (not x))");
    assert_eq!(one("!a && b"), "(&& (not a) b)");
}

#[test]
fn ternary_chains_to_the_right() {
    assert_eq!(one("a ? b : c"), "(?: a b c)");
    assert_eq!(one("a ? b : c ? d : e"), "(?: a b (?: c d e))");
    assert_eq!(
        one("x > 1 ? \"big\" : \"small\""),
        "(?: (> x 1) (str \"big\") (str \"small\"))"
    );
    assert_eq!(
        one("{a > 1 ? 1 : a > 0 ? 2 : 3}"),
        "(block (tail (?: (> a 1) 1 (?: (> a 0) 2 3))))"
    );
}

#[test]
fn question_mark_depends_on_spacing() {
    assert_eq!(one("f()?"), "(try (call f))");
    assert_eq!(one("a.b()? + 1"), "(+ (try (call (. a b))) 1)");
    assert_eq!(one("f()? ? a : b"), "(?: (try (call f)) a b)");
    assert_eq!(
        one("user?.name ?? \"x\""),
        "(?? (?. user name) (str \"x\"))"
    );
}

#[test]
fn assignment_and_wrapping() {
    assert_eq!(one("a = b = c"), "(= a (= b c))");
    assert_eq!(one("x += 1"), "(+= x 1)");
    assert_eq!(one("h *%= 31"), "(*%= h 31)");
    assert_eq!(one("v ??= 3"), "(??= v 3)");
}

#[test]
fn increment_is_a_statement() {
    assert_eq!(all("i++\nj--"), vec!["(++ i)", "(-- j)"]);
}

#[test]
fn ranges() {
    assert_eq!(one("0..10"), "(.. 0 10)");
    assert_eq!(one("0..=10"), "(..= 0 10)");
    assert_eq!(one("xs[2..]"), "(index xs (.. 2 _))");
    assert_eq!(one("xs[..3]"), "(index xs (.. _ 3))");
    assert_eq!(one("xs[-3..]"), "(index xs (.. (neg 3) _))");
    assert_eq!(one("0..10 step 2"), "(.. 0 10 step 2)");
    assert_eq!(one("0..n + 1"), "(.. 0 (+ n 1))");
    assert_eq!(one("x in 1..10 && y"), "(&& (in x (.. 1 10)) y)");
}

#[test]
fn conversions() {
    assert_eq!(one("x to float"), "(to x float)");
    assert_eq!(one("pet to? dog"), "(to? pet dog)");
    assert_eq!(one("a + b to float"), "(to (+ a b) float)");
    assert_eq!(one("x to float < 3.0"), "(< (to x float) 3.0)");
    assert_eq!(one("x to int ? a : b"), "(?: (to x int) a b)");
    // `to` as an ordinary name still works.
    assert_eq!(one("to + 1"), "(+ to 1)");
}

#[test]
fn calls_and_marks() {
    assert_eq!(one("f(1, 2)"), "(call f 1 2)");
    assert_eq!(one("c.bump!()"), "(call! (. c bump))");
    assert_eq!(one("c.close!!()"), "(call!! (. c close))");
    assert_eq!(one("a != b"), "(!= a b)");
    assert_eq!(one("a.b.c(1).d"), "(. (call (. (. a b) c) 1) d)");
    assert_eq!(one("clear(inout xs)"), "(call clear (arg inout xs))");
    assert_eq!(one("finish(sink xs)"), "(call finish (arg sink xs))");
    assert_eq!(
        one("greet(\"a\", greeting: \"hi\")"),
        "(call greet (str \"a\") (arg greeting: (str \"hi\")))"
    );
    assert_eq!(one("sum(...xs)"), "(call sum (arg ... xs))");
    assert_eq!(one("t.0.1"), "(.1 (.0 t))");
    assert_eq!(one("copy xs[1..3]"), "(copy (index xs (.. 1 3)))");
}

#[test]
fn generic_arguments_need_attached_angle_brackets() {
    assert_eq!(one("max<int>(1, 2)"), "(call (apply max int) 1 2)");
    assert_eq!(one("a < b"), "(< a b)");
    assert_eq!(one("a < b && c > (d)"), "(&& (< a b) (> c (paren d)))");
    assert_eq!(
        one("stack<int> { items: [] }"),
        "(new (stack int) (: items (list)))"
    );
}

#[test]
fn closing_angle_brackets_split() {
    assert_eq!(
        one("const m: map<string, list<int>> = {:}"),
        "(const m (: (map string (list int))) (map))"
    );
    assert_eq!(
        one("var v: list<list<int>>= none"),
        "(var v (: (list (list int))) none)"
    );
    assert_eq!(one("var w: list<int>= none"), "(var w (: (list int)) none)");
}

#[test]
fn struct_literals_and_blocks() {
    assert_eq!(one("point { x: 1, y: 2 }"), "(new point (: x 1) (: y 2))");
    assert_eq!(one("if x { y }"), "(if x (block (tail y)))");
    assert_eq!(
        one("if (point { x: 1 }) == p { y }"),
        "(if (== (paren (new point (: x 1))) p) (block (tail y)))"
    );
    assert_eq!(
        one("while ready { work() }"),
        "(while ready (block (tail (call work))))"
    );
    assert_eq!(one("for x of xs { x }"), "(for-of (x) xs (block (tail x)))");
    assert_eq!(one("shape.circle { r: 1 }"), "(new shape.circle (: r 1))");
}

#[test]
fn collection_literals() {
    assert_eq!(one("[1, 2, ...xs, 3]"), "(list 1 2 (... xs) 3)");
    assert_eq!(one("[0; 9]"), "(repeat 0 9)");
    assert_eq!(
        one("map { \"a\": 1, ...d }"),
        "(map (: (str \"a\") 1) (... d))"
    );
    assert_eq!(one("{:}"), "(map)");
    assert_eq!(one("set { 1, 2 }"), "(set 1 2)");
    assert_eq!(one("(1, 2)"), "(tuple 1 2)");
    assert_eq!(one("(1,)"), "(tuple 1)");
    assert_eq!(one("()"), "(tuple)");
}

#[test]
fn builders() {
    assert_eq!(one("[x of xs]"), "(build-list (of (x) xs))");
    assert_eq!(
        one("[x of xs ~> x * 2]"),
        "(build-list (of (x) xs) ~> (* x 2))"
    );
    assert_eq!(
        one("[x of xs if x > 0 ~> x * 2]"),
        "(build-list (of (x) xs) (if (> x 0)) ~> (* x 2))"
    );
    assert_eq!(
        one("[i, x of xs ~> (i, x)]"),
        "(build-list (of (i x) xs) ~> (tuple i x))"
    );
    assert_eq!(
        one("[x of xs, y of ys if x != y ~> (x, y)]"),
        "(build-list (of (x) xs) (of (y) ys) (if (!= x y)) ~> (tuple x y))"
    );
    assert_eq!(
        one("set { x of xs ~> x % 10 }"),
        "(build-set (of (x) xs) ~> (% x 10))"
    );
    assert_eq!(
        one("map { u of users ~> u.name : u.age }"),
        "(build-map (of (u) users) ~> (. u name) : (. u age))"
    );
    assert_eq!(
        one("sum(x of xs if x > 0 ~> x * x)"),
        "(call sum (build-stream (of (x) xs) (if (> x 0)) ~> (* x x)))"
    );
    assert_eq!(
        one("[x of xs.map(y ~> y + 1) ~> x]"),
        "(build-list (of (x) (call (. xs map) (~> (y) (+ y 1)))) ~> x)"
    );
    assert_eq!(
        one("[sink x of xs ~> consume(x)]"),
        "(build-list (of ((sink x)) xs) ~> (call consume x))"
    );
}

#[test]
fn closures() {
    assert_eq!(one("x ~> x * 2"), "(~> (x) (* x 2))");
    assert_eq!(one("~> it * 2"), "(~> () (* it 2))");
    assert_eq!(one("(a, b) ~> a + b"), "(~> (a b) (+ a b))");
    assert_eq!(
        one("(a: int, b: int) ~> a + b"),
        "(~> ((: a int) (: b int)) (+ a b))"
    );
    assert_eq!(
        one("[inout total] x ~> total += x"),
        "(~> (capture (inout total)) (x) (+= total x))"
    );
    assert_eq!(
        one("[inout n] ~> n++"),
        "(~> (capture (inout n)) () (++ n))"
    );
}

#[test]
fn closure_arguments_inside_calls() {
    assert_eq!(
        one("xs.map(x ~> x + 1).filter(~> it > 2)"),
        "(call (. (call (. xs map) (~> (x) (+ x 1))) filter) (~> () (> it 2)))"
    );
    assert_eq!(
        one("each([inout total] x ~> total += x, 5)"),
        "(call each (~> (capture (inout total)) (x) (+= total x)) 5)"
    );
}

#[test]
fn pipeline() {
    assert_eq!(one("raw |> trim |> parse"), "(|> (|> raw trim) parse)");
    assert_eq!(one("a ?? b |> f"), "(|> (?? a b) f)");
}

#[test]
fn strings_split_into_parts() {
    assert_eq!(one("\"hi\""), "(str \"hi\")");
    assert_eq!(one("\"a #b c\""), "(str \"a \" (# b) \" c\")");
    assert_eq!(one("\"#(1 + 2)!\""), "(str (# (+ 1 2)) \"!\")");
    assert_eq!(one("\"x\\#y\""), "(str \"x\\\\#y\")");
    assert_eq!(one("\"#a#b\""), "(str (# a) (# b))");
    assert_eq!(one("\"#(f(\"in\"))\""), "(str (# (call f (str \"in\"))))");
    assert_eq!(one("\"#(')')\""), "(str (# ')'))");
    assert_eq!(one("\"#é\""), "(str (# é))");
}

#[test]
fn errors_inside_interpolation_are_reported() {
    assert!(!errors("const s = \"a #(1 + ) b\"").is_empty());
}

#[test]
fn newlines_end_statements() {
    assert_eq!(all("a\nb"), vec!["a", "b"]);
    assert_eq!(all("a\n-b"), vec!["a", "(neg b)"]);
    assert_eq!(all("a\n(b)"), vec!["a", "(paren b)"]);
    assert_eq!(all("a;b"), vec!["a", "b"]);
}

#[test]
fn newlines_inside_brackets_are_ignored() {
    assert_eq!(one("f(\n1,\n2\n)"), "(call f 1 2)");
    assert_eq!(one("[\n1,\n2,\n]"), "(list 1 2)");
    assert_eq!(
        one("map {\n\"a\": 1,\n\"b\": 2\n}"),
        "(map (: (str \"a\") 1) (: (str \"b\") 2))"
    );
}

#[test]
fn lines_continue_after_operators_and_before_chains() {
    assert_eq!(one("a +\n b"), "(+ a b)");
    assert_eq!(one("a &&\n b"), "(&& a b)");
    assert_eq!(one("a\n  && b"), "(&& a b)");
    assert_eq!(
        one("xs\n  .map(f)\n  .to_list()"),
        "(call (. (call (. xs map) f) to_list))"
    );
    assert_eq!(
        one("raw\n  |> trim\n  |> parse"),
        "(|> (|> raw trim) parse)"
    );
    assert_eq!(one("c\n  ? a\n  : b"), "(?: c a b)");
    assert_eq!(one("x ??\n 3"), "(?? x 3)");
}

#[test]
fn blocks_have_a_tail_value() {
    assert_eq!(one("{ a; b }"), "(block a (tail b))");
    assert_eq!(one("{ a; b; }"), "(block a b)");
    assert_eq!(one("{ a\n b\n }"), "(block a (tail b))");
    assert_eq!(one("{ const x = 1 }"), "(block (const x 1))");
    assert_eq!(one("{}"), "(block)");
}

#[test]
fn declarations() {
    assert_eq!(one("const x = 1"), "(const x 1)");
    assert_eq!(one("var x: int = 1"), "(var x (: int) 1)");
    assert_eq!(
        one("const (q, r) = divmod(7, 2)"),
        "(const (tuple q r) (call divmod 7 2))"
    );
    assert_eq!(
        one("const point { x, y } = p"),
        "(const (struct point x y) p)"
    );
    assert_eq!(one("const [a, b] = pair"), "(const (list a b) pair)");
    assert_eq!(one("var x: int?"), "(var x (: (option int)))");
    assert_eq!(
        one("const r: int ! parse_error = ok(1)"),
        "(const r (: (result int parse_error)) (call ok 1))"
    );
}

#[test]
fn if_else_chains() {
    assert_eq!(
        one("if a { 1 } else if b { 2 } else { 3 }"),
        "(if a (block (tail 1)) (else (if b (block (tail 2)) (else (block (tail 3))))))"
    );
    assert_eq!(
        one("if a {\n 1\n}\nelse {\n 2\n}"),
        "(if a (block (tail 1)) (else (block (tail 2))))"
    );
    assert_eq!(all("if a { 1 }\nb"), vec!["(if a (block (tail 1)))", "b"]);
}

#[test]
fn loops() {
    assert_eq!(one("for i in 0..3 { }"), "(for-in i (.. 0 3) (block))");
    assert_eq!(one("for i, x of xs { }"), "(for-of (i x) xs (block))");
    assert_eq!(
        one("for inout x of xs { x++ }"),
        "(for-of ((inout x)) xs (block (tail (++ x))))"
    );
    assert_eq!(
        one("for (var i = 0; i < n; i += 1) { }"),
        "(for-c (var i 0) (< i n) (+= i 1) (block))"
    );
    assert_eq!(
        one("for (;;) { break }"),
        "(for-c _ _ _ (block (tail break)))"
    );
    assert_eq!(
        one("for (a, b) of pairs { }"),
        "(for-of ((tuple a b)) pairs (block))"
    );
    assert_eq!(one("do { x } while y"), "(do-while (block (tail x)) y)");
    assert_eq!(one("loop { break }"), "(loop (block (tail break)))");
    assert_eq!(one("while !done { }"), "(while (not done) (block))");
    assert_eq!(one("unless a { b }"), "(unless a (block (tail b)))");
}

#[test]
fn jumps() {
    assert_eq!(one("return"), "return");
    assert_eq!(one("return 1 + 2"), "(return (+ 1 2))");
    assert_eq!(one("{ return }"), "(block (tail return))");
    assert_eq!(one("become f(x - 1)"), "(become (call f (- x 1)))");
    assert_eq!(
        one("guard a is .b(c) else { return }"),
        "(guard (is a (.b c)) (block (tail return)))"
    );
    assert_eq!(one("x ?? return"), "(?? x return)");
    assert_eq!(one("defer close(f)"), "(defer (call close f))");
}

#[test]
fn switch_arms_end_at_the_next_case() {
    assert_eq!(
        one("switch x {\n case 1: a\n b\n case 2: c\n default: d\n}"),
        "(switch x (case (1) (block a (tail b))) (case (2) (block (tail c))) (case default (block (tail d))))"
    );
    assert_eq!(
        one("switch x { case 1: \"a\" case 2: \"b\" default: \"c\" }"),
        "(switch x (case (1) (block (tail (str \"a\")))) (case (2) (block (tail (str \"b\")))) (case default (block (tail (str \"c\")))))"
    );
    assert_eq!(
        one("const n = switch d {\n case 1: 10\n default: 0\n}"),
        "(const n (switch d (case (1) (block (tail 10))) (case default (block (tail 0)))))"
    );
}

#[test]
fn patterns() {
    let p = |src: &str| {
        let s = format!("switch v {{ case {src}: 0 }}");
        let (out, _) = parse_source(&s);
        assert!(out.errors.is_empty(), "{src}: {:?}", out.errors);
        let StmtKind::Expr(Expr {
            kind: ExprKind::Switch(sw),
            ..
        }) = &out.file.stmts[0].kind
        else {
            panic!("not a switch")
        };
        sw.arms[0]
            .patterns
            .iter()
            .map(|p| Printer { src: &s }.pattern(p).to_flat())
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(p("_"), "_");
    assert_eq!(p("n"), "n");
    assert_eq!(p("inout w"), "(inout w)");
    assert_eq!(p("1, 2, 3"), "1 2 3");
    assert_eq!(p("-1"), "(neg 1)");
    assert_eq!(p("\"start\""), "(str \"start\")");
    assert_eq!(p("'a'"), "'a'");
    assert_eq!(p("0..10"), "(.. 0 10)");
    assert_eq!(p("90..=100"), "(..= 90 100)");
    assert_eq!(p("10.."), "(.. 10 _)");
    assert_eq!(p("..0"), "(.. _ 0)");
    assert_eq!(p("(x, 0)"), "(tuple x 0)");
    assert_eq!(p(".circle(r)"), "(.circle r)");
    assert_eq!(p(".empty"), ".empty");
    assert_eq!(p("shape.rect(w: a, h)"), "(shape.rect (w: a) h)");
    assert_eq!(p("ok(.circle(r))"), "(ok (.circle r))");
    assert_eq!(p("none"), "none");
    assert_eq!(p("some(v)"), "(some v)");
    assert_eq!(p("d is dog"), "(as d (type dog))");
    assert_eq!(p("big is 100..=999"), "(as big (..= 100 999))");
    assert_eq!(p("point { x: 0, y }"), "(struct point (x 0) y)");
    assert_eq!(p("point { inout x, .. }"), "(struct point (inout x) ..)");
    assert_eq!(p("[]"), "(list)");
    assert_eq!(p("[a, ...rest]"), "(list a (... rest))");
    assert_eq!(p("[...init, last]"), "(list (... init) last)");
    assert_eq!(p("[first, ..., last]"), "(list first (...) last)");
    assert_eq!(
        p("[inout a, ...inout rest]"),
        "(list (inout a) (... inout rest))"
    );
    assert_eq!(p("[0, [x, ...]]"), "(list 0 (list x (...)))");
    assert_eq!(p("(p)"), "p");
}

#[test]
fn only_one_rest_in_a_list_pattern() {
    assert!(!errors("switch v { case [...a, ...b]: 0 }").is_empty());
}

#[test]
fn is_tests_types_and_patterns() {
    assert_eq!(one("pet is dog"), "(is pet (type dog))");
    assert_eq!(
        one("v is .circle(r) && r > 1"),
        "(&& (is v (.circle r)) (> r 1))"
    );
    assert_eq!(
        one("if xs is [h, ...] { h }"),
        "(if (is xs (list h (...))) (block (tail h)))"
    );
}

#[test]
fn types() {
    let t = |src: &str| {
        let printed = one(&format!("var x: {src}"));
        let inner = printed
            .strip_prefix("(var x (: ")
            .and_then(|s| s.strip_suffix("))"));
        inner.expect("a typed declaration").to_string()
    };
    assert_eq!(t("int"), "int");
    assert_eq!(t("[int]"), "(list int)");
    assert_eq!(t("[int; 9]"), "(array int 9)");
    assert_eq!(t("(int, string)"), "(tuple int string)");
    assert_eq!(t("(int, string; 4)"), "(tuple int string ; 4)");
    assert_eq!(t("int?"), "(option int)");
    assert_eq!(t("int ! parse_error"), "(result int parse_error)");
    assert_eq!(t("[int]?"), "(option (list int))");
    assert_eq!(t("map<string, [int]>"), "(map string (list int))");
    assert_eq!(t("fn(int, string) -> bool"), "(fn (int string) -> bool)");
    assert_eq!(t("once fn() -> int"), "(once-fn () -> int)");
    assert_eq!(t("once"), "once");
    assert_eq!(t("any shape_like"), "(any shape_like)");
    assert_eq!(t("soa<point, 8>"), "(soa point 8)");
    assert_eq!(t("std.io.file"), "std.io.file");
    assert_eq!(t("(int)"), "int");
}

#[test]
fn items() {
    assert_eq!(
        one("fn add(a: int, b: int) -> int { a + b }"),
        "(fn add ((a : int) (b : int)) -> int (block (tail (+ a b))))"
    );
    assert_eq!(
        one("fn double(x: int) -> int = x * 2"),
        "(fn double ((x : int)) -> int = (* x 2))"
    );
    assert_eq!(one("pub fn f() { }"), "(fn pub f () (block))");
    assert_eq!(one("pub pkg fn f() { }"), "(fn pub pkg f () (block))");
    assert_eq!(
        one("struct s { pub pkg x: int }"),
        "(struct s (field pub pkg x : int))"
    );
    assert_eq!(one("@inline fn f() { }"), "(fn (@ inline) f () (block))");
    assert_eq!(
        one("@derive(eq, hash)\nstruct id { value: int }"),
        "(struct (@ derive eq hash) id (field value : int))"
    );
    assert_eq!(
        one("meta fn table() -> int { 1 }"),
        "(fn meta table () -> int (block (tail 1)))"
    );
    assert_eq!(
        one("fn f<T: a + b>(x: T) { }"),
        "(fn f (generics (T a b)) ((x : T)) (block))"
    );
    assert_eq!(
        one("fn sum(...nums: int) -> int = 0"),
        "(fn sum ((... nums : int)) -> int = 0)"
    );
    assert_eq!(
        one("fn f(inout self, sink other: int) { }"),
        "(fn f ((inout self) (sink other : int)) (block))"
    );
    assert_eq!(one("mod tour"), "(mod tour)");
    assert_eq!(one("import std.io"), "(import std.io)");
    assert_eq!(one("import fmt = std.io"), "(import fmt = std.io)");
    assert_eq!(
        one("import { read, write: put } = std.io"),
        "(import ((: write put)) = std.io)".replace("((: write put))", "(read (: write put))")
    );
    assert_eq!(
        one("import { ... } = std.math"),
        "(import (...) = std.math)"
    );
    assert_eq!(
        one("import { a, ... } = .sibling"),
        "(import (a ...) = .sibling)"
    );
    assert_eq!(one("import ..util.strings"), "(import ..util.strings)");
    assert_eq!(one("import cs = ...up.x"), "(import cs = ...up.x)");
    assert_eq!(
        one("pub import { read } = std.io"),
        "(import pub (read) = std.io)"
    );
    assert_eq!(one("use std"), "(use std)");
    assert_eq!(one("type id = int64"), "(type id int64)");
    assert_eq!(
        one("impl shape_like for rect { }"),
        "(impl shape_like for rect)"
    );
    assert_eq!(
        one("enum shape { circle(radius: float)\n empty }"),
        "(enum shape (case circle (radius: float)) (case empty))"
    );
    assert_eq!(
        one("open obj animal { name: string }"),
        "(obj open animal (field name : string))"
    );
    assert_eq!(one("obj dog : animal { }"), "(obj dog (: animal))");
    assert_eq!(
        one("interface shape_like { fn area(self) -> float }"),
        "(interface shape_like (fn area (self) -> float))"
    );
    assert_eq!(one("test \"x\" { }"), "(test (str \"x\") (block))");
}

#[test]
fn soft_keywords_stay_usable_as_names() {
    assert_eq!(one("const from = 1"), "(const from 1)");
    assert_eq!(one("const step = to + it"), "(const step (+ to it))");
    assert_eq!(one("type(1)"), "(call type 1)");
    assert_eq!(one("open(file)"), "(call open file)");
    assert_eq!(one("test + 1"), "(+ test 1)");
    assert_eq!(one("const map = 1"), "(const map 1)");
    assert_eq!(one("map.get(1)"), "(call (. map get) 1)");
    assert_eq!(one("const default = 1"), "(const default 1)");
}

#[test]
fn scope_and_select() {
    assert_eq!(
        one("scope s { s.spawn(~> 1) }"),
        "(scope s (block (tail (call (. s spawn) (~> () 1)))))"
    );
    assert_eq!(
        one("select { case v = rx.recv() { f(v) } default { } }"),
        "(select (case (= v (call (. rx recv))) (block (tail (call f v)))) (case default (block)))"
    );
}

// ---- Errors -------------------------------------------------------------------

#[test]
fn errors_are_reported_with_help() {
    assert!(!errors("1 +").is_empty());
    assert!(!errors("f(1, 2").is_empty());
    assert!(!errors("if { }").is_empty());
    let e = errors("class Foo { }");
    assert!(e.iter().any(|m| m.contains("obj")), "{e:?}");
    let e = errors("null_thing = null");
    assert!(e.is_empty(), "{e:?}");
    let e = errors("async fn f() { }");
    assert!(e.is_empty(), "{e:?}");
    let e = errors("fn f() { await g() }");
    assert!(
        e.iter()
            .any(|m| m.contains("only be used inside an `async fn`")),
        "{e:?}"
    );
    let e = errors("async fn f() { await g() }");
    assert!(e.is_empty(), "{e:?}");
}

#[test]
fn parsing_continues_after_an_error() {
    let (out, _) = parse_source("const a = \nconst b = 2\nfoo(1,)\nconst c = 3");
    assert!(!out.errors.is_empty());
    let names: Vec<_> = out
        .file
        .stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Decl(d) => Some(Printer { src: "" }.pattern(&d.pattern).to_flat()),
            _ => None,
        })
        .collect();
    assert!(names.len() >= 2, "{names:?}");
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    // The parser recurses; give the test the stack the compiler itself runs on.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(deep_nesting_body)
        .unwrap()
        .join()
        .unwrap();
}

fn deep_nesting_body() {
    let src = "(".repeat(10_000);
    let (out, _) = parse_source(&src);
    assert!(!out.errors.is_empty());
    let src = format!("{}1{}", "[".repeat(5_000), "]".repeat(5_000));
    let (out, _) = parse_source(&src);
    assert!(!out.errors.is_empty());
    let src = "!".repeat(10_000);
    let (out, _) = parse_source(&src);
    assert!(!out.errors.is_empty());
    let src = format!("{}x", "-".repeat(10_000));
    let (out, _) = parse_source(&src);
    assert!(!out.errors.is_empty());
    let src = format!("{}{}", "if a { ".repeat(3_000), "}".repeat(3_000));
    let (out, _) = parse_source(&src);
    assert!(!out.errors.is_empty());
}

/// A small deterministic random number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SOUP: &[&str] = &[
    "fn",
    "struct",
    "enum",
    "obj",
    "impl",
    "interface",
    "const",
    "var",
    "if",
    "else",
    "unless",
    "switch",
    "case",
    "default",
    "for",
    "of",
    "in",
    "while",
    "do",
    "loop",
    "return",
    "break",
    "become",
    "guard",
    "is",
    "to",
    "meta",
    "pub",
    "self",
    "Self",
    "none",
    "true",
    "x",
    "y",
    "foo",
    "map",
    "set",
    "select",
    "scope",
    "test",
    "type",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ",",
    ";",
    ":",
    ".",
    "..",
    "...",
    "->",
    "~>",
    "|>",
    "?",
    "?.",
    "??",
    "!",
    "!!",
    "=",
    "+=",
    "<",
    ">",
    ">>",
    ">=",
    ">>=",
    "+",
    "-",
    "*",
    "/",
    "&&",
    "||",
    "1",
    "2.5",
    "\"s\"",
    "\"a #b\"",
    "\"#(1 +\"",
    "'c'",
    "_",
    "\n",
    "\n",
    " ",
    "@",
    "inout",
    "sink",
    "copy",
    "import",
    "mod",
    "from",
    "step",
];

#[test]
fn random_token_soup_never_panics_or_hangs() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..4_000 {
        let mut src = String::new();
        for _ in 0..rng.below(40) {
            src.push_str(SOUP[rng.below(SOUP.len())]);
            src.push(' ');
        }
        let (out, _) = parse_and_check(&src);
        // Printing must work on whatever tree came out.
        let _ = print_file(&out.file, &src);
    }
}

#[test]
fn damaged_real_files_never_panic_or_hang() {
    let tour =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/tour.pel"))
            .unwrap();
    let chars: Vec<char> = tour.chars().collect();
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..600 {
        let mut c = chars.clone();
        for _ in 0..1 + rng.below(6) {
            let i = rng.below(c.len());
            match rng.below(3) {
                0 => {
                    c.remove(i);
                }
                1 => c.insert(i, SOUP[rng.below(SOUP.len())].chars().next().unwrap_or(' ')),
                _ => {
                    let j = rng.below(c.len());
                    c.swap(i, j);
                }
            }
        }
        let src: String = c.into_iter().collect();
        let (out, _) = parse_and_check(&src);
        let _ = print_file(&out.file, &src);
    }
}

#[test]
fn truncated_real_files_never_panic_or_hang() {
    let tour =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/tour.pel"))
            .unwrap();
    let mut cut = 0;
    while cut < tour.len() {
        if tour.is_char_boundary(cut) {
            let (out, _) = parse_and_check(&tour[..cut]);
            let _ = print_file(&out.file, &tour[..cut]);
        }
        cut += 7;
    }
}

// ---- Labels, closure return types, doc comments -----------------------------------

#[test]
fn labels() {
    assert_eq!(
        one("outer: for x of xs { break outer }"),
        "(label outer (for-of (x) xs (block (tail (break outer)))))"
    );
    assert_eq!(
        one("a: loop { continue a }"),
        "(label a (loop (block (tail (continue a)))))"
    );
    assert_eq!(
        one("s: loop { break s 5 }"),
        "(label s (loop (block (tail (break s 5)))))"
    );
    // A name that is not a label is a value.
    assert_eq!(one("loop { break x }"), "(loop (block (tail (break x))))");
    // Parentheses force the value reading.
    assert_eq!(
        one("x: loop { break (x) }"),
        "(label x (loop (block (tail (break (paren x))))))"
    );
    assert_eq!(
        one("b: { break b 1 }"),
        "(label b (block (tail (break b 1))))"
    );
    // Labels do not reach into functions or closures.
    let (out, _) = parse_source("a: loop { f(~> break a) }");
    assert!(out.errors.is_empty());
    assert!(!checked("a: loop { f(~> break a) }").is_empty());
}

#[test]
fn a_label_cannot_be_reused_in_its_own_loop() {
    assert!(errors("a: loop { a: loop { } }")
        .iter()
        .any(|m| m.contains("already used")));
}

#[test]
fn closures_can_name_their_return_type() {
    assert_eq!(
        one("(a: int) -> int ~> a + 1"),
        "(~> ((: a int)) -> int (+ a 1))"
    );
    assert_eq!(
        one("[inout n] (a, b) -> [int] ~> { n += 1; [a, b] }"),
        "(~> (capture (inout n)) (a b) -> (list int) (block (+= n 1) (tail (list a b))))"
    );
    assert_eq!(
        one("(a) -> map<string, int> ~> m"),
        "(~> (a) -> (map string int) m)"
    );
    // Not a closure: no `~>` follows.
    assert!(!errors("(a, b) -> c").is_empty());
}

fn doc_of(src: &str) -> Vec<String> {
    let (out, _) = parse_source(src);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    out.file
        .stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Item(i) => Some(doc_text(src, &i.docs)),
            _ => None,
        })
        .collect()
}

#[test]
fn doc_comments_attach_to_the_next_declaration() {
    assert_eq!(doc_of("//~ one\n//~ two\nfn f() {}"), vec!["one\ntwo"]);
    assert_eq!(doc_of("//~ a\n\nfn f() {}\nfn g() {}"), vec!["a", ""]);
    assert_eq!(
        doc_of("//~* big\n   text *~//\nfn f() {}"),
        vec!["big\ntext"]
    );
    assert_eq!(doc_of("//~ for g\n@inline fn g() {}"), vec!["for g"]);
    assert_eq!(doc_of("//~ for g\npub fn g() {}"), vec!["for g"]);
}

#[test]
fn a_doc_comment_after_code_on_the_same_line_is_not_for_the_next_item() {
    assert_eq!(doc_of("const a = 1 //~ note\nfn f() {}"), vec![""]);
    assert_eq!(doc_of("fn f() {} //~ trailing\nfn g() {}"), vec!["", ""]);
}

#[test]
fn doc_comments_attach_to_fields_variants_and_methods() {
    let src = "struct s {\n  //~ the x\n  x: int\n  //~ a method\n  fn m(self) {}\n}\nenum e {\n  //~ first\n  a\n  b\n}";
    let (out, _) = parse_source(src);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    let StmtKind::Item(s) = &out.file.stmts[0].kind else {
        panic!()
    };
    let ItemKind::Struct(d) = &s.kind else {
        panic!()
    };
    let Member::Field(f) = &d.members[0] else {
        panic!()
    };
    assert_eq!(doc_text(src, &f.docs), "the x");
    let Member::Item(m) = &d.members[1] else {
        panic!()
    };
    assert_eq!(doc_text(src, &m.docs), "a method");
    let StmtKind::Item(e) = &out.file.stmts[1].kind else {
        panic!()
    };
    let ItemKind::Enum(d) = &e.kind else { panic!() };
    assert_eq!(doc_text(src, &d.variants[0].docs), "first");
    assert!(d.variants[1].docs.is_empty());
}

#[test]
fn struct_pattern_rest_comes_last() {
    assert!(errors("switch v { case point { .., x }: 0 }")
        .iter()
        .any(|m| m.contains("last")));
}

#[test]
fn builders_start_with_a_source() {
    assert!(!errors("[if x ~> x]").is_empty());
}

// ---- Syntax checks (check.rs) --------------------------------------------------------

fn checked(src: &str) -> Vec<String> {
    let (out, _) = parse_and_check(src);
    out.errors.into_iter().map(|e| e.message).collect()
}

fn assert_clean(src: &str) {
    let e = checked(src);
    assert!(e.is_empty(), "unexpected errors in {src:?}: {e:?}");
}

fn assert_flagged(src: &str, needle: &str) {
    let e = checked(src);
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "expected {needle:?} for {src:?}, got {e:?}"
    );
}

#[test]
fn break_and_continue_need_a_loop() {
    assert_flagged("break", "inside a loop");
    assert_flagged("continue", "inside a loop");
    assert_flagged("if a { break }", "inside a loop");
    assert_flagged("loop { f(~> break) }", "inside a loop");
    assert_flagged("fn f() { break }", "inside a loop");
    assert_clean("loop { break }");
    assert_clean("while a { if b { continue } }");
    assert_clean("for x of xs { switch x { case 1: break default: continue } }");
    assert_clean("a: { break a }");
}

#[test]
fn become_takes_a_call() {
    assert_clean("fn f(n: int) -> int = become f(n)");
    assert_flagged("fn f(n: int) -> int = become n", "function call");
    assert_flagged("fn f(n: int) -> int = become (f(n))", "function call");
}

#[test]
fn assignment_targets() {
    assert_clean("x = 1");
    assert_clean("a.b = 1");
    assert_clean("xs[0] += 1");
    assert_clean("t.0 = 1");
    assert_clean("_ = f()");
    assert_clean("(x) = 1");
    assert_clean("i++");
    assert_flagged("1 = 2", "cannot be assigned");
    assert_flagged("f() = 2", "cannot be assigned");
    assert_flagged("a?.b = 2", "cannot be assigned");
    assert_flagged("_ += 2", "cannot be assigned");
    assert_flagged("f()++", "`++`");
}

#[test]
fn self_and_special_functions() {
    assert_clean("struct s { fn m(self) {} fn n(inout self, x: int) {} fn assoc(x: int) {} }");
    assert_clean("enum e { a\n fn m(self) {} }");
    assert_clean("impl show for s { fn show(self) -> string = \"\" }");
    assert_clean("interface i { fn m(self) -> int }");
    assert_flagged("fn f(self) {}", "first parameter of a method");
    assert_flagged(
        "struct s { fn m(x: int, self) {} }",
        "first parameter of a method",
    );
    assert_flagged("fn f() { fn g(self) {} }", "first parameter of a method");
    assert_flagged("struct s { init(self) {} }", "no `self`");
    assert_clean("struct s { init(x: int) {} deinit {} }");
    assert_flagged("deinit {}", "belongs inside");
    assert_flagged("struct s { deinit(x: int) {} }", "cannot fail");
    assert_flagged("struct s { deinit() -> int {} }", "cannot fail");
    assert_flagged("interface i { deinit {} }", "belongs inside");
}

#[test]
fn parameters() {
    assert_clean("fn f(a: int, ...rest: int) {}");
    assert_flagged("fn f(...a: int, b: int) {}", "come last");
    assert_flagged("fn f(...a: int, ...b: int) {}", "come last");
    assert_flagged("fn f(a) {}", "needs a type");
    assert_flagged("fn f(a: int, a: int) {}", "appears twice");
    assert_clean("fn f(a: int = 1, b: string = \"x\") {}");
}

#[test]
fn function_bodies() {
    assert_flagged("fn f()", "needs a body");
    assert_clean("interface i { fn m(self)\n fn n(self) = 1 }");
    assert_clean("abstract obj a { abstract fn m(self) }");
    assert_clean("ext fn puts(s: ptr) -> int");
    assert_flagged("ext fn puts(s: ptr) {}", "has no body");
    assert_flagged("struct s { fn m(self) }", "needs a body");
    assert_flagged("impl i for s { fn m(self) }", "needs a body");
}

#[test]
fn declarations_need_a_value_and_no_modes() {
    assert_clean("const x = 1");
    assert_clean("const (a, b) = pair");
    assert_clean("ext const limit: int");
    assert_flagged("var x: int", "given a value");
    assert_flagged("ext const limit: int = 3", "no value");
    assert_flagged("const inout x = 1", "access modes");
    assert_flagged("const (a, sink b) = p", "access modes");
    assert_flagged("const 1 = x", "can fail");
    assert_flagged("const none = x", "can fail");
    assert_flagged("const (0, y) = p", "can fail");
    assert_flagged("const (a, a) = p", "bound twice");
}

#[test]
fn modifiers_fit_the_declaration() {
    assert_clean("open obj a { open fn m(self) {} }");
    assert_clean("obj b : a { override fn m(self) {} final fn n(self) {} }");
    assert_clean("linear struct t { x: int }");
    assert_clean("static var counter = 0");
    assert_flagged("open struct s {}", "`open` applies to");
    assert_flagged(
        "struct s { override fn m(self) {} }",
        "`override` applies to",
    );
    assert_flagged("linear fn f() {}", "`linear` applies to");
    assert_flagged("pub impl show for s {}", "do not apply");
    assert_flagged(
        "obj o { abstract fn m(self) }\nstruct s { abstract fn m(self) }",
        "`abstract` applies to",
    );
}

#[test]
fn interfaces_and_impls_have_no_fields() {
    assert_flagged("interface i { x: int }", "cannot have fields");
    assert_flagged("impl i for s { x: int }", "cannot have fields");
}

#[test]
fn pattern_bindings() {
    assert_flagged("switch v { case (a, a): 0 }", "bound twice");
    assert_flagged("for a, a of xs {}", "bound twice");
    assert_flagged("f((a, a) ~> 1)", "bound twice");
    assert_flagged("x is (a, a)", "bound twice");
    assert_clean("switch v { case (a, _): 0 case (_, _): 1 }");
}

#[test]
fn alternatives_bind_the_same_names() {
    assert_clean("switch s { case .circle(r), .square(r): r }");
    assert_clean("switch s { case 1, 2, 3: 0 }");
    assert_flagged("switch s { case .circle(r), .square(w): r }", "same names");
    assert_flagged("switch s { case .circle(r), .empty: r }", "same names");
    assert_flagged(
        "switch s { case .circle(inout r), .square(r): r }",
        "same names",
    );
}

#[test]
fn switch_defaults() {
    assert_clean("switch x { case 1: 0 default: 1 }");
    assert_flagged("switch x { default: 0 default: 1 }", "only one `default`");
    assert_flagged("switch x { default: 0 case 1: 1 }", "must be the last");
    assert_flagged("select { default { } default { } }", "only one `default`");
}

#[test]
fn map_builders_give_pairs() {
    assert_clean("map { x of xs ~> x : 1 }");
    assert_flagged("map { x of xs ~> x }", "key : value");
    assert_flagged("map { x of xs }", "key : value");
}

#[test]
fn repeated_names() {
    assert_flagged("point { x: 1, x: 2 }", "given twice");
    assert_flagged("f(a: 1, a: 2)", "given twice");
    assert_clean("f(a: 1, b: 2)");
}

#[test]
fn doc_comments_can_be_skipped() {
    let src = "//~ doc\nfn f() {}";
    let (kept, _) = parse_and_check_with(src, ParseOptions::default());
    let (skipped, _) = parse_and_check_with(src, ParseOptions::compile());
    let docs = |o: &ParseOutput| match &o.file.stmts[0].kind {
        StmtKind::Item(i) => i.docs.len(),
        _ => usize::MAX,
    };
    assert_eq!(docs(&kept), 1);
    assert_eq!(docs(&skipped), 0);
    assert!(skipped.errors.is_empty());
}

#[test]
fn import_checks() {
    assert_clean("import { a, b: c } = std.x");
    assert_clean("import { ... } = std.x");
    assert_flagged("import { } = std.x", "no names");
    assert_flagged("import { a, a } = std.x", "imported twice");
    assert_flagged("import { a, b: a } = std.x", "imported twice");
    assert_clean("import { a: x, b: y } = std.x");
}
